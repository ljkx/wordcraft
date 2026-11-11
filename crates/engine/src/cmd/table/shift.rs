//! Insert Cells and Delete Cells (#6330): the selected cells (the caret's without a selection)
//! go in or out and their neighbours shift right or down, or left or up; or whole rows or
//! columns go in or out, as in Word's two dialogs.
//!
//! - Right / left: each selected row gets as many new cells before its selected ones (or loses
//!   them), so it grows or shrinks at its end; the other rows stay as they are. Cells keep their
//!   widths: the grid is rebuilt from where the cells' edges now fall ([`regrid`]).
//! - Down / up: the text of the selected columns moves down (or up) by as many rows as are
//!   selected, from the selection's first row to the table's end. Down adds that many rows at the
//!   bottom of the table, so nothing is lost; up leaves empty cells at the bottom of the columns.
//!
//! A shift that would move part of a vertically merged cell, or move cells past cells of other
//! widths (down and up), is refused with a message, the table unchanged. So is moving a cell that
//! a content control around cells starts or ends at (the control would lose an end).

use std::collections::BTreeMap;

use super::cells::{cell, selected_cells};
use super::clear::{cell_prefix, guard_delete, text_spans};
use super::layout::{fix_caret, touch_cells};
use super::structure::{cols, del_col, del_row, delete_table_at};
use super::{CmdError, CmdResult, Paragraph, Session, Table, Value, p, para_block};
use wordcraft_doc::Cell;
use wordcraft_doc::props::VMerge;
use wordcraft_doc::table::{MAX_COLS, MAX_ROWS};

/// Insert Cells: `shift` is `right`, `down` (the default, as the dialog opens), `row` or `column`.
pub(super) fn insert_cells(s: &mut Session, v: &Value) -> CmdResult {
    match p::str(v, "shift") {
        Some("right") => shift_right(s),
        None | Some("down") => shift_down(s),
        Some("row") => insert_rows(s),
        Some("column") => insert_columns(s),
        Some(x) => Err(CmdError::Params(format!("unknown shift `{x}` (right, down, row or column)"))),
    }
}

/// Delete Cells: `shift` is `left` (the default), `up`, `row` or `column`.
pub(super) fn delete_cells(s: &mut Session, v: &Value) -> CmdResult {
    match p::str(v, "shift") {
        None | Some("left") => shift_left(s),
        Some("up") => shift_up(s),
        Some("row") => del_row(s, &Value::Null),
        Some("column") => del_col(s, &Value::Null),
        Some(x) => Err(CmdError::Params(format!("unknown shift `{x}` (left, up, row or column)"))),
    }
}

/// A refusal: the message goes to the status bar too, and nothing changes.
fn refuse(s: &mut Session, msg: &str) -> CmdError {
    s.status = msg.to_string();
    CmdError::Failed(msg.to_string())
}

const MERGED: &str = "Cells merged down can't be shifted; split them first.";
const CONTROL: &str = "A content control around cells is in the way; remove it first.";

/// The selected cells by row: the first and last selected cell of each row (cells the row
/// doesn't have are left out).
fn row_runs(s: &Session, t: &Table) -> BTreeMap<usize, (usize, usize)> {
    let mut runs: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
    for (r, c) in selected_cells(s) {
        if !t.rows.get(r).is_some_and(|row| c < row.cells.len()) {
            continue;
        }
        let e = runs.entry(r).or_insert((c, c));
        *e = (e.0.min(c), e.1.max(c));
    }
    runs
}

/// Where each row's cells stand, in points from the table's start: the row's first cell's
/// start and each cell's width, from the grid. A width the grid lacks or that is not usable
/// (a hostile file) counts as the last usable one, and every width as at least half a point,
/// so no two edges of a row meet.
struct Line {
    start: f32,
    widths: Vec<f32>,
}

const MIN_W: f32 = 0.5;
/// Edges this close are one edge.
const EPS: f32 = 0.05;

fn lines(t: &Table) -> Vec<Line> {
    let fallback = t.grid.iter().rev().copied().find(|w| w.is_finite() && *w > 0.0).unwrap_or(72.0);
    let col = |g: usize| t.grid.get(g).copied().filter(|w| w.is_finite() && *w > 0.0).unwrap_or(fallback).clamp(MIN_W, 1584.0);
    let sum = |a: usize, b: usize| (a..b).map(col).sum::<f32>();
    t.rows
        .iter()
        .map(|row| {
            let mut g = row.first_grid();
            let start = sum(0, g);
            let widths = row
                .cells
                .iter()
                .map(|c| {
                    let end = g.saturating_add(c.span()).min(MAX_COLS);
                    let w = sum(g, end).max(MIN_W);
                    g = end;
                    w
                })
                .collect();
            Line { start, widths }
        })
        .collect()
}

/// Rebuild the grid of `t` from `lines` (one per row, a width per cell): its columns are the
/// gaps between all the edges the rows' cells have, each cell spans the columns between its
/// edges, and the columns before and after a row are its `gridBefore` and `gridAfter`. Fails,
/// leaving `t` alone, when that takes more columns than a table may have.
fn regrid(t: &mut Table, lines: &[Line]) -> Result<(), CmdError> {
    let mut edges = vec![0.0f32];
    for l in lines {
        let mut x = l.start;
        edges.push(x);
        for w in &l.widths {
            x += w;
            edges.push(x);
        }
    }
    edges.retain(|x| x.is_finite());
    edges.sort_by(f32::total_cmp);
    edges.dedup_by(|a, b| (*a - *b).abs() < EPS);
    let n = edges.len().saturating_sub(1);
    if n == 0 || n > MAX_COLS {
        return Err(CmdError::Failed(format!("A table can have at most {MAX_COLS} columns.")));
    }
    let at = |x: f32| edges.iter().enumerate().min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs())).map_or(0, |(i, _)| i).min(n);
    let old_total: f32 = t.grid.iter().filter(|w| w.is_finite()).sum();
    t.grid = edges.windows(2).map(|w| if let [a, b] = w { b - a } else { 0.0 }).collect();
    let gap = |a: usize, b: usize| t.grid.get(a..b).map_or(0.0, |g| g.iter().sum::<f32>());
    let mut afters = Vec::with_capacity(t.rows.len());
    for (row, l) in t.rows.iter_mut().zip(lines) {
        let mut i = at(l.start);
        row.props.grid_before = i as u32;
        let mut x = l.start;
        for (c, w) in row.cells.iter_mut().zip(&l.widths) {
            x += w;
            let j = at(x).max(i + 1).min(MAX_COLS);
            c.props.span = j.saturating_sub(i).max(1) as u32;
            i = j;
        }
        let after = n.saturating_sub(i);
        row.props.grid_after = after as u32;
        afters.push((i, after));
    }
    // The widths of the gaps after rows follow their columns.
    for (row, (i, after)) in t.rows.iter_mut().zip(afters) {
        if row.props.w_after.is_some() {
            row.props.w_after = (after > 0).then(|| gap(i, i + after));
        }
    }
    // A table with a preferred width keeps its columns' proportions: the width grows or shrinks
    // with the grid, as the row now reaches further out (or not as far).
    let new_total: f32 = t.grid.iter().sum();
    if let Some(w) = t.props.width
        && old_total > 0.0
        && w.is_finite()
    {
        t.props.width = Some((w * new_total / old_total).clamp(0.0, 1584.0 * 4.0));
    }
    Ok(())
}

/// Insert Cells › Shift cells right: before each selected row's first selected cell, as many
/// empty cells like the selected ones; the row's cells from there move right.
fn shift_right(s: &mut Session) -> CmdResult {
    let (tp, _, _) = cell(s)?;
    let story = s.sel.focus.story;
    let t = s.doc.table(story, &tp).cloned().ok_or_else(|| CmdError::Failed("no table".into()))?;
    let runs = row_runs(s, &t);
    if moved_merged(&t, &runs) {
        return Err(refuse(s, MERGED));
    }
    let mut t = t;
    let mut lines = lines(&t);
    for (&r, &(lo, hi)) in &runs {
        let (Some(row), Some(line)) = (t.rows.get_mut(r), lines.get_mut(r)) else { continue };
        let new: Vec<Cell> = row.cells.get(lo..=hi).unwrap_or_default().iter().map(blank_like).collect();
        let widths: Vec<f32> = line.widths.get(lo..=hi).unwrap_or_default().to_vec();
        row.cells.splice(lo..lo, new);
        line.widths.splice(lo..lo, widths);
    }
    regrid(&mut t, &lines).map_err(|e| refuse(s, &e.to_string()))?;
    *s.doc.table_mut(story, &tp)? = t;
    touch_cells(s, &tp)?;
    let (r, c) = runs.iter().next().map_or((0, 0), |(&r, &(lo, _))| (r, lo));
    fix_caret(s, &tp, r, c)
}

/// Delete Cells › Shift cells left: each selected row loses its selected cells; the cells after
/// them move left. A row left without cells goes, and the table with its last row. With Track
/// Changes on, the cells' text is marked deleted and the cells stay (as for Delete Rows).
fn shift_left(s: &mut Session) -> CmdResult {
    let (tp, _, _) = cell(s)?;
    let story = s.sel.focus.story;
    let t = s.doc.table(story, &tp).cloned().ok_or_else(|| CmdError::Failed("no table".into()))?;
    let runs = row_runs(s, &t);
    // Every selected row whole: that is Delete Rows (the table, when it is every row).
    if !runs.is_empty() && runs.iter().all(|(&r, &(lo, hi))| lo == 0 && t.rows.get(r).is_some_and(|row| hi + 1 >= row.cells.len())) {
        return del_row(s, &Value::Null);
    }
    if moved_merged(&t, &runs) {
        return Err(refuse(s, MERGED));
    }
    let removed = |r: usize, lo: usize, hi: usize| t.rows.get(r).and_then(|row| row.cells.get(lo..=hi)).unwrap_or_default();
    if runs.iter().any(|(&r, &(lo, hi))| removed(r, lo, hi).iter().any(|c| !c.controls.is_empty())) {
        return Err(refuse(s, CONTROL));
    }
    let prefixes: Vec<Vec<u32>> =
        runs.iter().flat_map(|(&r, &(lo, hi))| (lo..=hi).map(move |c| (r, c))).map(|(r, c)| cell_prefix(&tp, r, c)).collect();
    let spans = text_spans(s, story, &prefixes);
    if let Some(done) = guard_delete(s, &spans)? {
        return Ok(done);
    }
    let Some(t) = s.doc.table(story, &tp) else { return Err(CmdError::Failed("no table".into())) };
    let mut t = t.clone();
    let mut lines = lines(&t);
    for (&r, &(lo, hi)) in &runs {
        let (Some(row), Some(line)) = (t.rows.get_mut(r), lines.get_mut(r)) else { continue };
        if hi < row.cells.len() && hi < line.widths.len() {
            row.cells.drain(lo..=hi);
            line.widths.drain(lo..=hi);
        }
    }
    // Rows without cells go (bottom up, so the indices hold), their lines with them.
    for r in (0..t.rows.len()).rev() {
        if t.rows.get(r).is_some_and(|row| row.cells.is_empty()) {
            t.delete_row(r);
            if r < lines.len() {
                lines.remove(r);
            }
        }
    }
    if t.rows.is_empty() {
        return delete_table_at(s, &tp);
    }
    regrid(&mut t, &lines).map_err(|e| refuse(s, &e.to_string()))?;
    *s.doc.table_mut(story, &tp)? = t;
    touch_cells(s, &tp)?;
    let (r, c) = runs.iter().next().map_or((0, 0), |(&r, &(lo, _))| (r, lo));
    fix_caret(s, &tp, r, c)
}

/// Whether a shift in `runs` moves a cell merged down (its other part would stay put).
fn moved_merged(t: &Table, runs: &BTreeMap<usize, (usize, usize)>) -> bool {
    runs.iter()
        .any(|(&r, &(lo, _))| t.rows.get(r).and_then(|row| row.cells.get(lo..)).unwrap_or_default().iter().any(|c| c.props.vmerge != VMerge::None))
}

/// The selection as Shift cells down or up sees it: its first row, how many rows it covers,
/// and for each row from its first to the table's last the cells of its columns. Every such row
/// must have cells there with the same edges, none merged down and none a content control around
/// cells starts or ends at; otherwise the message why not.
struct Block {
    r0: usize,
    n: usize,
    /// Per row from `r0` on: the cell indices of the selected columns, left to right.
    cells: Vec<Vec<usize>>,
}

fn column_block(s: &Session, t: &Table) -> Result<Block, &'static str> {
    const UNEVEN: &str = "The cells below the selection don't line up with it; they can't be shifted.";
    let mut picked: Vec<(usize, usize)> =
        selected_cells(s).into_iter().filter(|&(r, c)| t.rows.get(r).is_some_and(|row| c < row.cells.len())).collect();
    picked.sort_unstable();
    let (Some(r0), Some(r1)) = (picked.iter().map(|x| x.0).min(), picked.iter().map(|x| x.0).max()) else { return Err(UNEVEN) };
    // The grid columns the selected cells cover.
    let (mut g0, mut g1) = (usize::MAX, 0usize);
    for &(r, c) in &picked {
        let g = t.grid_col(r, c);
        let span = t.rows.get(r).and_then(|row| row.cells.get(c)).map_or(1, Cell::span);
        g0 = g0.min(g);
        g1 = g1.max(g.saturating_add(span));
    }
    let mut shape: Option<Vec<(usize, usize)>> = None;
    let mut cells = Vec::new();
    for row in t.rows.iter().skip(r0) {
        let mut g = row.first_grid();
        let (mut edges, mut idx) = (Vec::new(), Vec::new());
        for (c, x) in row.cells.iter().enumerate() {
            let end = g.saturating_add(x.span());
            if end > g0 && g < g1 {
                if g < g0 || end > g1 {
                    return Err(UNEVEN);
                }
                if x.props.vmerge != VMerge::None {
                    return Err(MERGED);
                }
                if !x.controls.is_empty() {
                    return Err(CONTROL);
                }
                edges.push((g, end));
                idx.push(c);
            }
            g = end;
        }
        if edges.is_empty() || shape.as_ref().is_some_and(|s| *s != edges) {
            return Err(UNEVEN);
        }
        shape = Some(edges);
        cells.push(idx);
    }
    Ok(Block { r0, n: r1 - r0 + 1, cells })
}

/// The cell's text (and anything else in it), leaving it one empty paragraph.
fn take_content(c: &mut Cell) -> wordcraft_doc::Blocks {
    std::mem::replace(&mut c.blocks, vec![para_block(Paragraph::new())])
}

/// Insert Cells › Shift cells down: the text of the selected columns, from the selection's
/// first row on, moves down by as many rows as are selected, and as many rows are added at the
/// bottom of the table to take the last of it. The selected cells are left empty.
fn shift_down(s: &mut Session) -> CmdResult {
    let (tp, _, _) = cell(s)?;
    let story = s.sel.focus.story;
    let t = s.doc.table(story, &tp).cloned().ok_or_else(|| CmdError::Failed("no table".into()))?;
    let b = match column_block(s, &t) {
        Ok(b) => b,
        Err(m) => return Err(refuse(s, m)),
    };
    if t.rows.len().saturating_add(b.n) > MAX_ROWS {
        return Err(refuse(s, &format!("A table can have at most {MAX_ROWS} rows.")));
    }
    let mut t = t;
    let last = t.rows.len().saturating_sub(1);
    for _ in 0..b.n {
        t.insert_row(t.rows.len(), last);
        // The new rows don't continue a merge the last row is part of.
        if let Some(row) = t.rows.last_mut() {
            row.cells.iter_mut().for_each(|c| c.props.vmerge = VMerge::None);
        }
    }
    // The new rows are like the last, so they have its cells in the selected columns.
    let mut cells = b.cells;
    let tail = cells.last().cloned().unwrap_or_default();
    cells.extend(std::iter::repeat_n(tail, b.n));
    let rows = cells.len();
    for k in 0..cells.first().map_or(0, Vec::len) {
        for i in (0..rows).rev() {
            let content = if i >= b.n { slot(&mut t, b.r0 + i - b.n, &cells, i - b.n, k).map(take_content) } else { None };
            if let Some(c) = slot(&mut t, b.r0 + i, &cells, i, k) {
                c.blocks = content.unwrap_or_else(|| vec![para_block(Paragraph::new())]);
            }
        }
    }
    *s.doc.table_mut(story, &tp)? = t;
    touch_cells(s, &tp)?;
    fix_caret(s, &tp, b.r0, cells.first().and_then(|v| v.first()).copied().unwrap_or(0))
}

/// Delete Cells › Shift cells up: the selected cells' text goes, and the text below them in
/// their columns moves up by as many rows as are selected, leaving that many empty cells at the
/// bottom of the columns. With Track Changes on, the text is marked deleted and nothing moves.
fn shift_up(s: &mut Session) -> CmdResult {
    let (tp, _, _) = cell(s)?;
    let story = s.sel.focus.story;
    let t = s.doc.table(story, &tp).cloned().ok_or_else(|| CmdError::Failed("no table".into()))?;
    let b = match column_block(s, &t) {
        Ok(b) => b,
        Err(m) => return Err(refuse(s, m)),
    };
    let prefixes: Vec<Vec<u32>> =
        b.cells.iter().take(b.n).enumerate().flat_map(|(i, v)| v.iter().map(move |&c| (b.r0 + i, c))).map(|(r, c)| cell_prefix(&tp, r, c)).collect();
    let spans = text_spans(s, story, &prefixes);
    if let Some(done) = guard_delete(s, &spans)? {
        return Ok(done);
    }
    let Some(t) = s.doc.table(story, &tp) else { return Err(CmdError::Failed("no table".into())) };
    let mut t = t.clone();
    let rows = b.cells.len();
    for k in 0..b.cells.first().map_or(0, Vec::len) {
        for i in 0..rows {
            let content = if i + b.n < rows { slot(&mut t, b.r0 + i + b.n, &b.cells, i + b.n, k).map(take_content) } else { None };
            if let Some(c) = slot(&mut t, b.r0 + i, &b.cells, i, k) {
                c.blocks = content.unwrap_or_else(|| vec![para_block(Paragraph::new())]);
            }
        }
    }
    *s.doc.table_mut(story, &tp)? = t;
    touch_cells(s, &tp)?;
    fix_caret(s, &tp, b.r0, b.cells.first().and_then(|v| v.first()).copied().unwrap_or(0))
}

/// Cell `k` of the selected columns in row `r` (the block's row `i`).
fn slot<'a>(t: &'a mut Table, r: usize, cells: &[Vec<usize>], i: usize, k: usize) -> Option<&'a mut Cell> {
    let c = *cells.get(i)?.get(k)?;
    t.rows.get_mut(r)?.cells.get_mut(c)
}

/// An empty cell with `c`'s formatting, not merged.
fn blank_like(c: &Cell) -> Cell {
    let mut nc = Cell::empty();
    nc.props = c.props.clone();
    nc.props.vmerge = VMerge::None;
    nc
}

/// Insert Cells › Insert entire row: as many rows as are selected, above the selection's first,
/// like it.
fn insert_rows(s: &mut Session) -> CmdResult {
    let (tp, _, _) = cell(s)?;
    let story = s.sel.focus.story;
    let t = s.doc.table(story, &tp).cloned().ok_or_else(|| CmdError::Failed("no table".into()))?;
    let picked: std::collections::BTreeSet<usize> = selected_cells(s).into_iter().map(|x| x.0).filter(|&r| r < t.rows.len()).collect();
    let r0 = picked.first().copied().unwrap_or(0);
    let n = picked.len().max(1);
    if t.rows.len().saturating_add(n) > MAX_ROWS {
        return Err(refuse(s, &format!("A table can have at most {MAX_ROWS} rows.")));
    }
    let t = s.doc.table_mut(story, &tp)?;
    for _ in 0..n {
        t.insert_row(r0, r0);
    }
    fix_caret(s, &tp, r0, 0)
}

/// Insert Cells › Insert entire column: as many columns as the most cells selected in a row,
/// left of the selection.
fn insert_columns(s: &mut Session) -> CmdResult {
    let (tp, _, _) = cell(s)?;
    let story = s.sel.focus.story;
    let t = s.doc.table(story, &tp).cloned().ok_or_else(|| CmdError::Failed("no table".into()))?;
    let runs = row_runs(s, &t);
    let n = runs.values().map(|&(lo, hi)| hi + 1 - lo).max().unwrap_or(1);
    // The leftmost selected cell (by grid column), in the first row that has it.
    let (r, c) = runs.iter().map(|(&r, &(lo, _))| (r, lo)).min_by_key(|&(r, c)| (t.grid_col(r, c), r)).unwrap_or((0, 0));
    if t.grid.len().saturating_add(n) > MAX_COLS {
        return Err(refuse(s, &format!("A table can have at most {MAX_COLS} columns.")));
    }
    cols(s, &tp, r, c, false, n)
}
