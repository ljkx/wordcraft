//! #6330: Insert Cells and Delete Cells with Word's shift options, on the selected cells.

use serde_json::{Value, json};
use wordcraft_doc::props::VMerge;
use wordcraft_doc::{Path, Pos, StoryRef, Table};

use crate::{Selection, Session};

fn run(s: &mut Session, id: &str, v: Value) -> Value {
    s.run(id, &v).unwrap_or_else(|e| panic!("{id} {v}: {e}"))
}

fn table(s: &Session) -> &Table {
    s.doc.body.iter().find_map(|b| b.as_table()).expect("a table")
}

/// Every cell's text, row by row.
fn cells(s: &Session) -> Vec<Vec<String>> {
    table(s)
        .rows
        .iter()
        .map(|r| r.cells.iter().map(|c| c.blocks.iter().filter_map(|b| b.as_para()).map(|p| p.plain_text()).collect()).collect())
        .collect()
}

/// A `rows` × `cols` table whose cells read A1, B1, … (column letter, row number).
fn grid_session(rows: usize, cols: usize) -> Session {
    let mut s = Session::new(wordcraft_doc::Document::new());
    run(&mut s, "insert.table", json!({"rows": rows, "cols": cols}));
    for r in 0..rows {
        for c in 0..cols {
            if r + c > 0 {
                run(&mut s, "text.tab", json!({}));
            }
            let name = format!("{}{}", char::from(b'A' + c as u8), r + 1);
            run(&mut s, "text.insert", json!({"text": name}));
        }
    }
    caret_in(&mut s, 0, 0);
    s
}

fn caret_in(s: &mut Session, r: u32, c: u32) {
    s.sel = Selection::caret(Pos { story: StoryRef::Body, path: Path(vec![0, r, c, 0]), off: 0 });
}

/// Select whole cells: rows and grid columns, both inclusive.
fn select(s: &mut Session, rows: (usize, usize), cols: (usize, usize)) {
    crate::cmd::table::select_range(s, StoryRef::Body, &Path(vec![0]), rows, cols).unwrap();
}

fn texts(rows: &[&[&str]]) -> Vec<Vec<String>> {
    rows.iter().map(|r| r.iter().map(|t| t.to_string()).collect()).collect()
}

/// The table is whole: its grid has a column for every grid column a row reaches, and each row
/// covers the grid exactly (`gridBefore`, its cells' spans, `gridAfter`).
fn assert_consistent(t: &Table) {
    assert!(!t.rows.is_empty());
    for (i, row) in t.rows.iter().enumerate() {
        assert!(!row.cells.is_empty(), "row {i} has no cells");
        let reach = row.first_grid() + row.cells.iter().map(|c| c.span()).sum::<usize>() + row.grid_after();
        assert_eq!(reach, t.grid.len(), "row {i} covers {reach} of {} grid columns", t.grid.len());
    }
}

/// Each cell's width in points, row by row: the grid columns it spans.
fn widths(t: &Table) -> Vec<Vec<f32>> {
    t.rows
        .iter()
        .map(|row| {
            let mut g = row.first_grid();
            row.cells
                .iter()
                .map(|c| {
                    let w: f32 = t.grid[g..g + c.span()].iter().sum();
                    g += c.span();
                    (w * 10.0).round() / 10.0
                })
                .collect()
        })
        .collect()
}

#[test]
fn delete_cells_shift_left_takes_the_selected_cells_of_each_row() {
    let mut s = grid_session(3, 3);
    let w = table(&s).grid[0];
    select(&mut s, (0, 1), (1, 1));
    run(&mut s, "table.deleteCells", json!({"shift": "left"}));
    assert_eq!(cells(&s), texts(&[&["A1", "C1"], &["A2", "C2"], &["A3", "B3", "C3"]]));
    let t = table(&s);
    assert_consistent(t);
    // The cells keep their widths; the shortened rows end a column early.
    assert!(widths(t).iter().flatten().all(|x| (x - (w * 10.0).round() / 10.0).abs() < 0.2), "{:?}", widths(t));
    assert_eq!(t.rows.iter().map(|r| r.props.grid_after).collect::<Vec<_>>(), [1, 1, 0]);
    // The caret is in the first row, where the cells went.
    assert_eq!(s.sel.focus.path.cell().map(|(_, r, c)| (r, c)), Some((0, 1)));
}

#[test]
fn delete_cells_without_a_shift_takes_the_caret_cell_and_shifts_left() {
    let mut s = grid_session(2, 3);
    caret_in(&mut s, 1, 0);
    run(&mut s, "table.deleteCells", json!({}));
    assert_eq!(cells(&s), texts(&[&["A1", "B1", "C1"], &["B2", "C2"]]));
    assert_consistent(table(&s));
}

/// A row whose every cell goes goes with them, as Delete Rows; the last row takes the table.
#[test]
fn delete_cells_of_a_whole_row_deletes_the_row() {
    let mut s = grid_session(2, 1);
    run(&mut s, "table.deleteCells", json!({"shift": "left"}));
    assert_eq!(cells(&s), texts(&[&["A2"]]));
    run(&mut s, "table.deleteCells", json!({"shift": "left"}));
    assert!(s.doc.body.iter().all(|b| b.as_table().is_none()), "the last cell takes the table");
    run(&mut s, "edit.undo", json!({}));
    assert_eq!(cells(&s), texts(&[&["A2"]]));
}

/// The last cell of a row goes without moving anything; the row ends a column early.
#[test]
fn delete_cells_on_the_last_cell_of_a_row() {
    let mut s = grid_session(2, 2);
    caret_in(&mut s, 0, 1);
    run(&mut s, "table.deleteCells", json!({"shift": "left"}));
    assert_eq!(cells(&s), texts(&[&["A1"], &["A2", "B2"]]));
    assert_eq!(table(&s).rows[0].props.grid_after, 1);
    assert_consistent(table(&s));
}

#[test]
fn insert_cells_shift_right_adds_cells_before_the_selected_ones() {
    let mut s = grid_session(3, 3);
    select(&mut s, (0, 1), (1, 1));
    run(&mut s, "table.insertCells", json!({"shift": "right"}));
    assert_eq!(cells(&s), texts(&[&["A1", "", "B1", "C1"], &["A2", "", "B2", "C2"], &["A3", "B3", "C3"]]));
    let t = table(&s);
    assert_consistent(t);
    assert_eq!(t.grid.len(), 4, "the longer rows reach a fourth column");
    assert_eq!(t.rows[2].props.grid_after, 1);
    assert_eq!(s.sel.focus.path.cell().map(|(_, r, c)| (r, c)), Some((0, 1)), "the caret goes to the first new cell");
}

/// Cells of different widths keep theirs: the grid is rebuilt from where their edges fall.
#[test]
fn shifting_right_and_left_keeps_every_cell_width() {
    let mut s = grid_session(3, 3);
    {
        let t = s.doc.table_mut(StoryRef::Body, &Path(vec![0])).unwrap();
        t.grid = vec![50.0, 100.0, 150.0];
        crate::cmd::table::sync_cell_widths(t);
    }
    caret_in(&mut s, 0, 1);
    run(&mut s, "table.insertCells", json!({"shift": "right"}));
    let t = table(&s);
    assert_consistent(t);
    assert_eq!(widths(t), [vec![50.0, 100.0, 100.0, 150.0], vec![50.0, 100.0, 150.0], vec![50.0, 100.0, 150.0]]);
    assert_eq!(t.grid, [50.0, 100.0, 100.0, 50.0, 100.0]);
    // The row's last cell spans the columns its edges take in.
    assert_eq!(t.rows[1].cells[2].span(), 2);

    caret_in(&mut s, 2, 0);
    run(&mut s, "table.deleteCells", json!({"shift": "left"}));
    let t = table(&s);
    assert_consistent(t);
    assert_eq!(widths(t), [vec![50.0, 100.0, 100.0, 150.0], vec![50.0, 100.0, 150.0], vec![100.0, 150.0]]);
}

#[test]
fn insert_cells_shift_down_moves_the_columns_down_and_adds_rows_at_the_bottom() {
    let mut s = grid_session(3, 3);
    caret_in(&mut s, 0, 1);
    run(&mut s, "table.insertCells", json!({"shift": "down"}));
    assert_eq!(cells(&s), texts(&[&["A1", "", "C1"], &["A2", "B1", "C2"], &["A3", "B2", "C3"], &["", "B3", ""]]));
    assert_consistent(table(&s));

    // Two rows and two columns selected: both columns move two rows down; two rows are added.
    let mut s = grid_session(3, 3);
    select(&mut s, (0, 1), (1, 2));
    run(&mut s, "table.insertCells", json!({"shift": "down"}));
    assert_eq!(cells(&s), texts(&[&["A1", "", ""], &["A2", "", ""], &["A3", "B1", "C1"], &["", "B2", "C2"], &["", "B3", "C3"]]));
    assert_consistent(table(&s));
    assert_eq!(s.sel.focus.path.cell().map(|(_, r, c)| (r, c)), Some((0, 1)));
}

/// Without `shift` Insert Cells shifts down, as its dialog opens.
#[test]
fn insert_cells_without_a_shift_shifts_down() {
    let mut s = grid_session(2, 2);
    run(&mut s, "table.insertCells", json!({}));
    assert_eq!(cells(&s), texts(&[&["", "B1"], &["A1", "B2"], &["A2", ""]]));
}

#[test]
fn delete_cells_shift_up_moves_the_text_below_up() {
    let mut s = grid_session(3, 3);
    caret_in(&mut s, 0, 1);
    run(&mut s, "table.deleteCells", json!({"shift": "up"}));
    assert_eq!(cells(&s), texts(&[&["A1", "B2", "C1"], &["A2", "B3", "C2"], &["A3", "", "C3"]]));
    assert_consistent(table(&s));

    let mut s = grid_session(4, 2);
    select(&mut s, (1, 2), (0, 1));
    run(&mut s, "table.deleteCells", json!({"shift": "up"}));
    assert_eq!(cells(&s), texts(&[&["A1", "B1"], &["A4", "B4"], &["", ""], &["", ""]]));
}

#[test]
fn insert_entire_rows_and_columns_go_before_the_selection() {
    let mut s = grid_session(3, 3);
    // Selected from the bottom up: the rows still go above the selection's first row.
    let at = |r, c| Pos { story: StoryRef::Body, path: Path(vec![0, r, c, 0]), off: 0 };
    s.sel = Selection { anchor: at(2, 2), focus: at(1, 1) };
    run(&mut s, "table.insertCells", json!({"shift": "row"}));
    assert_eq!(cells(&s).iter().map(|r| r.join(",")).collect::<Vec<_>>(), ["A1,B1,C1", ",,", ",,", "A2,B2,C2", "A3,B3,C3"]);

    let mut s = grid_session(2, 3);
    let total: f32 = table(&s).grid.iter().sum();
    select(&mut s, (0, 0), (1, 2));
    run(&mut s, "table.insertCells", json!({"shift": "column"}));
    assert_eq!(cells(&s), texts(&[&["A1", "", "", "B1", "C1"], &["A2", "", "", "B2", "C2"]]));
    let t = table(&s);
    assert_consistent(t);
    assert!((t.grid.iter().sum::<f32>() - total).abs() < 0.1, "the table keeps its width");
}

#[test]
fn delete_entire_rows_and_columns() {
    let mut s = grid_session(3, 3);
    select(&mut s, (1, 2), (1, 1));
    run(&mut s, "table.deleteCells", json!({"shift": "row"}));
    assert_eq!(cells(&s), texts(&[&["A1", "B1", "C1"]]));

    let mut s = grid_session(2, 3);
    select(&mut s, (0, 0), (0, 1));
    run(&mut s, "table.deleteCells", json!({"shift": "column"}));
    assert_eq!(cells(&s), texts(&[&["C1"], &["C2"]]));
}

/// Each option is one step to undo, and the undo gives back the table as it was.
#[test]
fn every_shift_undoes_in_one_step() {
    for (id, shift) in [
        ("table.insertCells", "right"),
        ("table.insertCells", "down"),
        ("table.insertCells", "row"),
        ("table.insertCells", "column"),
        ("table.deleteCells", "left"),
        ("table.deleteCells", "up"),
        ("table.deleteCells", "row"),
        ("table.deleteCells", "column"),
    ] {
        let mut s = grid_session(3, 3);
        select(&mut s, (0, 1), (1, 2));
        let before = s.doc.clone();
        run(&mut s, id, json!({"shift": shift}));
        assert_ne!(s.doc, before, "{id} {shift} changed nothing");
        run(&mut s, "edit.undo", json!({}));
        assert_eq!(s.doc, before, "{id} {shift}: one undo restores the table");
    }
}

/// A shift that would move part of a vertically merged cell, or cells past cells of another
/// width, is refused with a message, and the table stays as it was.
#[test]
fn shifts_that_would_break_merged_cells_are_refused() {
    let refused = |s: &mut Session, id: &str, shift: &str| {
        let before = s.doc.clone();
        assert!(s.run(id, &json!({"shift": shift})).is_err(), "{id} {shift} went ahead");
        assert_eq!(s.doc, before, "{id} {shift} changed the table");
        assert!(!s.status.is_empty(), "{id} {shift} says why");
    };
    // B1 and B2 merged down.
    let mut s = grid_session(3, 3);
    select(&mut s, (0, 1), (1, 1));
    run(&mut s, "table.merge", json!({}));
    assert_eq!(table(&s).rows[1].cells[1].props.vmerge, VMerge::Continue);
    caret_in(&mut s, 0, 1);
    refused(&mut s, "table.deleteCells", "up");
    refused(&mut s, "table.insertCells", "down");
    caret_in(&mut s, 1, 0);
    refused(&mut s, "table.insertCells", "right");
    refused(&mut s, "table.deleteCells", "left");
    // Column A and the cells right of the merge move freely.
    run(&mut s, "table.insertCells", json!({"shift": "down"}));
    assert_eq!(cells(&s).iter().map(|r| r[0].as_str()).collect::<Vec<_>>(), ["A1", "", "A2", "A3"]);
    caret_in(&mut s, 2, 2);
    run(&mut s, "table.deleteCells", json!({"shift": "left"}));
    assert_consistent(table(&s));

    // B1 and C1 merged across: the cells below don't line up with it.
    let mut s = grid_session(3, 3);
    select(&mut s, (0, 0), (1, 2));
    run(&mut s, "table.merge", json!({}));
    caret_in(&mut s, 0, 1);
    refused(&mut s, "table.insertCells", "down");
    refused(&mut s, "table.deleteCells", "up");
    // A merged cell moves right whole, as wide as it was; the new cell is as wide.
    let w = widths(table(&s))[0][1];
    run(&mut s, "table.insertCells", json!({"shift": "right"}));
    let t = table(&s);
    assert_consistent(t);
    assert_eq!(widths(t)[0][1..], [w, w]);
    assert_eq!(cells(&s)[0].len(), 3);
}

#[test]
fn an_unknown_shift_is_refused() {
    let mut s = grid_session(2, 2);
    let before = s.doc.clone();
    for (id, shift) in [("table.insertCells", "left"), ("table.deleteCells", "down"), ("table.deleteCells", "sideways")] {
        assert!(matches!(s.run(id, &json!({"shift": shift})), Err(crate::CmdError::Params(_))), "{id} {shift}");
    }
    assert_eq!(s.doc, before);
}

/// With Track Changes on, Delete Cells marks the cells' text deleted and moves nothing, as the
/// other table deletes do; Reject All brings it back.
#[test]
fn tracked_delete_cells_marks_the_text_deleted() {
    for shift in ["left", "up"] {
        let mut s = grid_session(2, 2);
        run(&mut s, "review.trackChanges", json!({"value": true}));
        run(&mut s, "table.deleteCells", json!({"shift": shift}));
        assert_eq!(cells(&s), texts(&[&["A1", "B1"], &["A2", "B2"]]), "{shift} moved cells");
        let p = table(&s).rows[0].cells[0].blocks[0].as_para().unwrap().clone();
        assert!(p.run_ranges().all(|(_, c)| c.del.is_some()), "{shift}: A1 not marked deleted");
        run(&mut s, "review.rejectAll", json!({}));
        assert!(s.doc.revisions.is_empty(), "{shift}");
    }
}

/// Ragged tables, hostile grids, spans and merges from a file: every option, every selection,
/// never a panic, and the table stays whole.
#[test]
fn shifts_on_hostile_tables_never_panic() {
    let shapes: [fn(&mut Table); 6] = [
        |t| t.grid = vec![f32::NAN, 0.0, -5.0],
        |t| t.grid = vec![1e30, 1e-9, 72.0],
        |t| {
            t.rows[1].cells.truncate(1);
            t.rows[0].props.grid_before = 2;
        },
        |t| {
            t.rows[0].cells[0].props.span = u32::MAX;
            t.rows[2].props.grid_after = 99;
        },
        |t| {
            t.rows[0].cells[2].props.vmerge = VMerge::Continue;
            t.rows[2].cells[0].props.vmerge = VMerge::Restart;
        },
        |t| t.grid.clear(),
    ];
    let shifts = [("table.insertCells", ["right", "down", "row", "column"]), ("table.deleteCells", ["left", "up", "row", "column"])];
    for (k, shape) in shapes.iter().enumerate() {
        for (id, opts) in shifts {
            for shift in opts {
                for (rows, cols) in [((0, 0), (0, 0)), ((0, 2), (1, 2)), ((1, 2), (0, 0)), ((0, 2), (0, 2))] {
                    let mut s = grid_session(3, 3);
                    shape(s.doc.table_mut(StoryRef::Body, &Path(vec![0])).unwrap());
                    let _ = crate::cmd::table::select_range(&mut s, StoryRef::Body, &Path(vec![0]), rows, cols);
                    let _ = s.run(id, &json!({"shift": shift}));
                    s.clamp_selection();
                    let _ = s.layout();
                    if let Some(t) = s.doc.body.iter().find_map(|b| b.as_table()) {
                        assert!(!t.rows.is_empty() && t.rows.iter().all(|r| !r.cells.is_empty()), "shape {k}, {id} {shift}");
                    }
                }
            }
        }
    }
}
