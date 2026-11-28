//! Tables in the formatted composer: inserted, grown and shrunk a row or
//! column at a time, their columns given widths.
//!
//! A table is the run of blocks whose paths share its `<table>` element;
//! its rows are the `<tr>` elements in those paths and its cells the `<td>`
//! (or `<th>`) after each row's. Rows and cells are found by their elements
//! (`Rc` identity), as everywhere in the document, so edits made here build
//! new elements and leave the rest of the table, and what another app put
//! on it, as it was.
use super::*;
use std::ops::Range;

/// A table as rows of cells, by block.
#[derive(Clone, Debug)]
pub struct Table {
    /// Where its element is in its blocks' paths.
    pub k: usize,
    /// Its blocks.
    pub blocks: Range<usize>,
    pub rows: Vec<Row>,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub tr: El,
    /// Where the row's element is in its blocks' paths.
    pub r: usize,
    pub cells: Vec<Cell>,
}

#[derive(Clone, Debug)]
pub struct Cell {
    pub el: El,
    /// Where the cell's element is in its blocks' paths.
    pub c: usize,
    pub blocks: Range<usize>,
    /// Columns it spans (`colspan`).
    pub span: usize,
}

impl Row {
    /// Each cell's first column.
    pub fn starts(&self) -> Vec<usize> {
        let mut at = 0;
        self.cells
            .iter()
            .map(|c| {
                let start = at;
                at += c.span;
                start
            })
            .collect()
    }
    pub fn columns(&self) -> usize {
        self.cells.iter().map(|c| c.span).sum()
    }
    /// The cell covering column `col`, and its first column.
    fn covering(&self, col: usize) -> Option<(usize, usize)> {
        self.starts().into_iter().enumerate().find(|&(i, s)| col >= s && col < s + self.cells[i].span)
    }
    fn blocks(&self) -> Range<usize> {
        let start = self.cells.first().map_or(0, |c| c.blocks.start);
        let end = self.cells.last().map_or(start, |c| c.blocks.end);
        start..end
    }
}

impl Table {
    pub fn columns(&self) -> usize {
        self.rows.iter().map(Row::columns).max().unwrap_or(0)
    }
    /// The row and cell block `bi` is in.
    pub fn locate(&self, bi: usize) -> Option<(usize, usize)> {
        for (ri, row) in self.rows.iter().enumerate() {
            for (ci, cell) in row.cells.iter().enumerate() {
                if cell.blocks.contains(&bi) {
                    return Some((ri, ci));
                }
            }
        }
        None
    }
    /// Each column's width as its cells say it (the first cell of one
    /// column that does).
    pub fn widths(&self) -> Vec<Option<Width>> {
        let mut out = vec![None; self.columns()];
        for row in &self.rows {
            for (cell, start) in row.cells.iter().zip(row.starts()) {
                if cell.span == 1
                    && let Some(slot) = out.get_mut(start)
                    && slot.is_none()
                {
                    *slot = cell_width(&cell.el);
                }
            }
        }
        out
    }
}

/// A column's width, as a table's HTML gives it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Width {
    /// Of the table's width.
    Percent(f32),
    /// CSS pixels.
    Px(f32),
}

/// The width a cell's style or `width` attribute gives it.
pub fn cell_width(e: &Element) -> Option<Width> {
    let read = |v: &str| {
        let v = v.trim().trim_end_matches("!important").trim();
        if let Some(p) = v.strip_suffix('%') {
            return p.trim().parse::<f32>().ok().filter(|n| *n > 0. && n.is_finite()).map(|n| Width::Percent(n.min(100.)));
        }
        let n = v.strip_suffix("px").unwrap_or(v).trim();
        n.parse::<f32>().ok().filter(|n| *n > 0. && n.is_finite()).map(Width::Px)
    };
    e.style()
        .iter()
        .find(|(n, _)| n == "width")
        .and_then(|(_, v)| read(v))
        .or_else(|| e.attr("width").and_then(read))
}

/// Columns' widths in pixels in a table `total` wide: those the table
/// gives (in percent or pixels), the rest sharing what's left equally;
/// scaled down together when they'd be wider than the table.
pub fn column_widths(widths: &[Option<Width>], total: f32) -> Vec<f32> {
    let n = widths.len().max(1);
    let mut given: Vec<Option<f32>> = widths
        .iter()
        .map(|w| match w {
            Some(Width::Percent(p)) => Some(total * p / 100.),
            Some(Width::Px(px)) => Some(*px),
            None => None,
        })
        .collect();
    given.resize(n, None);
    let min = 24f32.min(total / n as f32);
    let unset = given.iter().filter(|w| w.is_none()).count();
    let fixed: f32 = given.iter().flatten().sum();
    // What's left for the others, at least a narrow column each.
    let rest = (total - fixed).max(min * unset as f32);
    let mut out: Vec<f32> = given.iter().map(|w| w.unwrap_or(rest / unset.max(1) as f32).max(min)).collect();
    let sum: f32 = out.iter().sum();
    if sum > total && sum > 0. {
        for w in &mut out {
            *w *= total / sum;
        }
    } else if unset == 0 && sum < total && sum > 0. {
        // All given and narrower: the table still fills its width, as the
        // editor shows it, the columns in their proportions.
        for w in &mut out {
            *w *= total / sum;
        }
    }
    out
}

/// The innermost table cell element in `path`, if any.
pub fn cell_index(path: &[El]) -> Option<usize> {
    path.iter().rposition(|e| matches!(e.tag.as_str(), "td" | "th"))
}

/// Whether two blocks are in the same table cell (or both outside any).
pub fn same_cell(a: &[El], b: &[El]) -> bool {
    match (cell_index(a), cell_index(b)) {
        (None, None) => true,
        (Some(i), Some(j)) => Rc::ptr_eq(&a[i], &b[j]),
        _ => false,
    }
}

/// The style new tables' cells get, so they show as a grid in every app.
const CELL_STYLE: &str = "border:1px solid #c8c8c8;padding:4px 8px;vertical-align:top";
const TABLE_STYLE: &str = "border-collapse:collapse;width:100%";

/// A copy of cell `e` for a new cell beside or below it: its look, not its
/// span or width.
fn new_cell_like(e: &Element) -> El {
    let mut attrs: Vec<(String, String)> = e
        .attrs
        .iter()
        .filter(|(n, _)| !matches!(n.as_str(), "colspan" | "rowspan" | "width" | "id"))
        .cloned()
        .collect();
    if let Some(i) = attrs.iter().position(|(n, _)| n == "style") {
        let kept: Vec<(String, String)> = css::declarations(&attrs[i].1).into_iter().filter(|(n, _)| n != "width").collect();
        if kept.is_empty() {
            attrs.remove(i);
        } else {
            attrs[i].1 = css::style_attr(&kept);
        }
    }
    Rc::new(Element { tag: e.tag.clone(), attrs })
}

fn with_colspan(e: &Element, span: usize) -> El {
    let mut n = e.clone();
    n.attrs.retain(|(a, _)| a != "colspan");
    if span > 1 {
        n.attrs.push(("colspan".into(), span.to_string()));
    }
    Rc::new(n)
}

impl Doc {
    /// The innermost table block `bi` is in.
    pub fn table_at(&self, bi: usize) -> Option<Table> {
        let path = &self.blocks.get(bi)?.path;
        let c = cell_index(path)?;
        let k = path[..c].iter().rposition(|e| e.is("table"))?;
        self.table_with(bi, k)
    }

    /// The table whose element is at `k` in block `bi`'s path.
    pub fn table_with(&self, bi: usize, k: usize) -> Option<Table> {
        let table = self.blocks.get(bi)?.path.get(k).filter(|e| e.is("table"))?.clone();
        let inside = |j: usize| self.blocks[j].path.get(k).is_some_and(|e| Rc::ptr_eq(e, &table));
        let mut start = bi;
        while start > 0 && inside(start - 1) {
            start -= 1;
        }
        let mut end = bi + 1;
        while end < self.blocks.len() && inside(end) {
            end += 1;
        }
        let mut rows: Vec<Row> = Vec::new();
        for (j, block) in self.blocks.iter().enumerate().take(end).skip(start) {
            let path = &block.path;
            let Some(r) = (k + 1..path.len()).find(|&r| path[r].is("tr")) else { continue };
            let Some(c) = (r + 1..path.len()).find(|&c| matches!(path[c].tag.as_str(), "td" | "th")) else { continue };
            if !rows.last().is_some_and(|row| Rc::ptr_eq(&row.tr, &path[r])) {
                rows.push(Row { tr: path[r].clone(), r, cells: Vec::new() });
            }
            let row = rows.last_mut().expect("a row");
            match row.cells.last_mut() {
                Some(cell) if Rc::ptr_eq(&cell.el, &path[c]) => cell.blocks.end = j + 1,
                _ => {
                    let span = path[c].attr("colspan").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1).clamp(1, 20);
                    row.cells.push(Cell { el: path[c].clone(), c, blocks: j..j + 1, span });
                }
            }
        }
        if rows.is_empty() {
            return None;
        }
        Some(Table { k, blocks: start..end, rows })
    }

    /// Inserts a table of `rows` by `cols` empty cells at `pos`: in place
    /// of an empty line, else after the line (split there); a line to go
    /// on writing on follows it. The start of its first cell.
    pub fn insert_table(&mut self, pos: Pos, rows: usize, cols: usize) -> Pos {
        let (rows, cols) = (rows.clamp(1, 50), cols.clamp(1, 20));
        let pos = self.clamp(pos);
        let current = &self.blocks[pos.block];
        let own = current.element().clone();
        // Where the table goes: beside the line, inside what holds it (a
        // quote, a cell), not inside a list.
        let mut container = current.path.clone();
        if own.is_plain_block() || own.is("li") || own.is("hr") {
            container.pop();
        }
        while container.last().is_some_and(|e| matches!(e.tag.as_str(), "li" | "ul" | "ol")) {
            container.pop();
        }
        let replace = current.is_empty() && !current.is_rule();
        // In place of an empty line; before a line when at its start; else
        // after it, splitting it where the caret is.
        let (first, line_after) = if replace {
            (pos.block, true)
        } else if pos.offset == 0 && !current.is_rule() {
            (pos.block, false)
        } else if pos.offset < current.len() && !current.is_rule() {
            self.split(pos);
            (pos.block + 1, false)
        } else {
            (pos.block + 1, true)
        };
        let table = Rc::new(Element { tag: "table".into(), attrs: vec![("style".into(), TABLE_STYLE.into())] });
        let body = Element::new("tbody");
        let cell = Rc::new(Element { tag: "td".into(), attrs: vec![("style".into(), CELL_STYLE.into())] });
        let mut blocks = Vec::with_capacity(rows * cols + 1);
        for _ in 0..rows {
            let tr = Element::new("tr");
            for _ in 0..cols {
                let mut path = container.clone();
                path.extend([table.clone(), body.clone(), tr.clone(), cell.copy()]);
                blocks.push(Rc::new(Block { path, runs: vec![] }));
            }
        }
        if line_after {
            // Somewhere to go on writing after it.
            let mut path = container.clone();
            path.push(Element::new("div"));
            blocks.push(Rc::new(Block { path, runs: vec![] }));
        }
        let removed = if replace { first..first + 1 } else { first..first };
        self.blocks.splice(removed, blocks);
        Pos::new(first, 0)
    }

    /// Adds a row above or below block `bi`'s, its cells like that row's;
    /// the start of the new row's first cell.
    pub fn table_insert_row(&mut self, bi: usize, below: bool) -> Option<Pos> {
        let t = self.table_at(bi)?;
        let (ri, _) = t.locate(bi)?;
        let row = &t.rows[ri];
        let tr = row.tr.copy();
        let mut blocks = Vec::new();
        for cell in &row.cells {
            let source = &self.blocks[cell.blocks.start].path;
            let mut path = source[..row.r].to_vec();
            path.push(tr.clone());
            path.push(with_colspan(&new_cell_like(&cell.el), cell.span));
            blocks.push(Rc::new(Block { path, runs: vec![] }));
        }
        let at = if below { row.blocks().end } else { row.blocks().start };
        self.blocks.splice(at..at, blocks);
        Some(Pos::new(at, 0))
    }

    /// Removes block `bi`'s row (the whole table with its last row); where
    /// the caret goes.
    pub fn table_delete_row(&mut self, bi: usize) -> Option<Pos> {
        let t = self.table_at(bi)?;
        if t.rows.len() <= 1 {
            return self.table_delete(bi);
        }
        let (ri, ci) = t.locate(bi)?;
        let range = t.rows[ri].blocks();
        self.blocks.drain(range.clone());
        // The same column of the row that took its place (or the one above).
        let t = self.table_at(range.start.min(self.blocks.len() - 1)).or_else(|| self.table_at(range.start.saturating_sub(1)))?;
        let row = &t.rows[ri.min(t.rows.len() - 1)];
        let cell = &row.cells[ci.min(row.cells.len() - 1)];
        Some(Pos::new(cell.blocks.start, 0))
    }

    /// Adds a column left or right of block `bi`'s cell; the start of the
    /// new cell in its row.
    pub fn table_insert_column(&mut self, bi: usize, right: bool) -> Option<Pos> {
        let t = self.table_at(bi)?;
        let (ri, ci) = t.locate(bi)?;
        let start = t.rows[ri].starts()[ci];
        let col = if right { start + t.rows[ri].cells[ci].span } else { start };
        let mut caret = None;
        // Last row first, so earlier blocks keep their places.
        for (rj, row) in t.rows.iter().enumerate().rev() {
            let source = &self.blocks[row.cells[0].blocks.start].path;
            let prefix = source[..row.r + 1].to_vec();
            match row.covering(col) {
                Some((i, s)) if s == col => {
                    let mut path = prefix;
                    path.push(new_cell_like(&row.cells[i].el));
                    let at = row.cells[i].blocks.start;
                    self.blocks.insert(at, Rc::new(Block { path, runs: vec![] }));
                    if rj == ri {
                        caret = Some(at);
                    }
                }
                Some((i, _)) => {
                    // Inside a cell spanning columns: it spans one more.
                    let cell = &row.cells[i];
                    let wider = with_colspan(&cell.el, cell.span + 1);
                    for j in cell.blocks.clone() {
                        self.block_mut(j).path[cell.c] = wider.clone();
                    }
                    if rj == ri {
                        caret = Some(cell.blocks.start);
                    }
                }
                None => {
                    let last = row.cells.last().expect("a cell");
                    let mut path = prefix;
                    path.push(new_cell_like(&last.el));
                    let at = last.blocks.end;
                    self.blocks.insert(at, Rc::new(Block { path, runs: vec![] }));
                    if rj == ri {
                        caret = Some(at);
                    }
                }
            }
        }
        let _ = caret?;
        let t = self.table_at(t.blocks.start)?;
        let row = &t.rows[ri];
        let (i, _) = row.covering(col).unwrap_or((row.cells.len() - 1, 0));
        Some(Pos::new(row.cells[i].blocks.start, 0))
    }

    /// Removes block `bi`'s column (the whole table with its last column);
    /// where the caret goes.
    pub fn table_delete_column(&mut self, bi: usize) -> Option<Pos> {
        let t = self.table_at(bi)?;
        if t.columns() <= 1 {
            return self.table_delete(bi);
        }
        let (ri, ci) = t.locate(bi)?;
        let col = t.rows[ri].starts()[ci];
        for row in t.rows.iter().rev() {
            let Some((i, _)) = row.covering(col) else { continue };
            let cell = &row.cells[i];
            if cell.span > 1 {
                let narrower = with_colspan(&cell.el, cell.span - 1);
                for j in cell.blocks.clone() {
                    self.block_mut(j).path[cell.c] = narrower.clone();
                }
            } else {
                self.blocks.drain(cell.blocks.clone());
            }
        }
        // A row left without cells goes too (its blocks are gone already).
        let t = self.table_at(t.blocks.start.min(self.blocks.len() - 1))?;
        let row = &t.rows[ri.min(t.rows.len() - 1)];
        let cell = &row.cells[ci.min(row.cells.len() - 1)];
        Some(Pos::new(cell.blocks.start, 0))
    }

    /// Removes block `bi`'s table; the start of the line in its place.
    pub fn table_delete(&mut self, bi: usize) -> Option<Pos> {
        let t = self.table_at(bi)?;
        let mut path = self.blocks[t.blocks.start].path[..t.k].to_vec();
        path.push(Element::new("div"));
        self.blocks.splice(t.blocks.clone(), [Rc::new(Block { path, runs: vec![] })]);
        Some(Pos::new(t.blocks.start, 0))
    }

    /// Gives block `bi`'s table's columns these widths (percent of the
    /// table, one per column), on every cell of each column; `None` takes
    /// the widths off (equal columns).
    pub fn set_table_widths(&mut self, bi: usize, widths: Option<&[f32]>) -> bool {
        let Some(t) = self.table_at(bi) else { return false };
        for row in &t.rows {
            for (cell, start) in row.cells.iter().zip(row.starts()) {
                let value = match widths {
                    Some(w) if cell.span == 1 => w.get(start).map(|p| format!("{}%", (p * 10.).round() / 10.)),
                    Some(w) => {
                        let sum: f32 = w.iter().skip(start).take(cell.span).sum();
                        (sum > 0.).then(|| format!("{}%", (sum * 10.).round() / 10.))
                    }
                    None => None,
                };
                let mut e = (*cell.el.with_style("width", value.as_deref())).clone();
                e.attrs.retain(|(n, _)| n != "width");
                let e = Rc::new(e);
                for j in cell.blocks.clone() {
                    self.block_mut(j).path[cell.c] = e.clone();
                }
            }
        }
        true
    }

    /// Block `bi`'s table's columns' widths in percent, as shown (each
    /// column's share of `total` pixels).
    pub fn table_percents(&self, bi: usize) -> Option<Vec<f32>> {
        let t = self.table_at(bi)?;
        let total = 1000.;
        let px = column_widths(&t.widths(), total);
        Some(px.iter().map(|w| w / total * 100.).collect())
    }

    /// The start of the cell after (or before) block `bi`'s, if any.
    pub fn table_step(&self, bi: usize, forward: bool) -> Option<Pos> {
        let t = self.table_at(bi)?;
        let (ri, ci) = t.locate(bi)?;
        let flat: Vec<(usize, usize)> = t.rows.iter().enumerate().flat_map(|(r, row)| (0..row.cells.len()).map(move |c| (r, c))).collect();
        let i = flat.iter().position(|&x| x == (ri, ci))?;
        let next = if forward { flat.get(i + 1) } else { i.checked_sub(1).and_then(|j| flat.get(j)) }?;
        let cell = &t.rows[next.0].cells[next.1];
        Some(if forward {
            Pos::new(cell.blocks.start, 0)
        } else {
            Pos::new(cell.blocks.end - 1, self.blocks[cell.blocks.end - 1].len())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::html::{parse, to_html};
    use super::*;

    fn texts(doc: &Doc) -> Vec<String> {
        doc.blocks.iter().map(|b| b.text()).collect()
    }

    fn grid(doc: &Doc, bi: usize) -> Vec<Vec<String>> {
        let t = doc.table_at(bi).expect("a table");
        t.rows
            .iter()
            .map(|r| r.cells.iter().map(|c| c.blocks.clone().map(|j| doc.blocks[j].text()).collect::<Vec<_>>().join("/")).collect())
            .collect()
    }

    #[test]
    fn a_table_is_inserted_with_a_line_after_it() {
        let mut doc = Doc::from_text("Hello");
        let at = doc.insert_table(Pos::new(0, 5), 2, 3);
        assert_eq!(at, Pos::new(1, 0));
        assert_eq!(doc.blocks.len(), 1 + 6 + 1);
        let t = doc.table_at(1).unwrap();
        assert_eq!((t.rows.len(), t.columns()), (2, 3));
        assert!(doc.table_at(7).is_none(), "the line after it is outside");
        let at = doc.insert_text(at, "a", Style::plain());
        doc.insert_text(Pos::new(2, 0), "b", Style::plain());
        assert_eq!(at, Pos::new(1, 1));
        let html = to_html(&doc);
        assert!(html.contains("<table style=\"border-collapse:collapse;width:100%\"><tbody><tr><td style="), "{html}");
        // Read back, the same grid.
        let back = parse(&html, false);
        assert_eq!(grid(&back, 1), [["a", "b", ""], ["", "", ""]]);
        // An empty line is replaced.
        let mut empty = Doc::empty();
        let at = empty.insert_table(Pos::default(), 1, 2);
        assert_eq!(at, Pos::new(0, 0));
        assert_eq!(empty.blocks.len(), 3);
    }

    #[test]
    fn rows_and_columns_come_and_go() {
        let mut doc = parse("<p>x</p><table><tr><td>a</td><td>b</td></tr><tr><td>c</td><td>d</td></tr></table><p>y</p>", false);
        assert_eq!(grid(&doc, 1), [["a", "b"], ["c", "d"]]);
        let at = doc.table_insert_row(1, true).unwrap();
        assert_eq!(grid(&doc, 1), [["a", "b"], ["", ""], ["c", "d"]]);
        assert_eq!(at, Pos::new(3, 0));
        doc.table_insert_row(1, false).unwrap();
        assert_eq!(grid(&doc, 1), [["", ""], ["a", "b"], ["", ""], ["c", "d"]]);
        let b = texts(&doc).iter().position(|t| t == "b").unwrap();
        doc.table_insert_column(b, false).unwrap();
        assert_eq!(grid(&doc, 1), [["", "", ""], ["a", "", "b"], ["", "", ""], ["c", "", "d"]]);
        let d = texts(&doc).iter().position(|t| t == "d").unwrap();
        doc.table_insert_column(d, true).unwrap();
        assert_eq!(grid(&doc, 1)[3], ["c", "", "d", ""]);
        let a = texts(&doc).iter().position(|t| t == "a").unwrap();
        doc.table_delete_column(a + 1).unwrap();
        assert_eq!(grid(&doc, 1), [["", "", ""], ["a", "b", ""], ["", "", ""], ["c", "d", ""]]);
        doc.table_delete_row(1).unwrap();
        doc.table_delete_row(texts(&doc).iter().position(|t| t == "a").unwrap() + 3).unwrap();
        assert_eq!(grid(&doc, 1), [["a", "b", ""], ["c", "d", ""]]);
        assert_eq!(texts(&doc).first().unwrap(), "x");
        assert_eq!(texts(&doc).last().unwrap(), "y");
        // The table goes with its last row.
        doc.table_delete_row(1).unwrap();
        let at = doc.table_delete_row(1).unwrap();
        assert!(doc.table_at(at.block).is_none());
        assert_eq!(texts(&doc), ["x", "", "y"]);
    }

    #[test]
    fn spanning_cells_widen_and_narrow() {
        let mut doc = parse("<table><tr><td colspan=\"2\">wide</td></tr><tr><td>a</td><td>b</td></tr></table>", false);
        let b = texts(&doc).iter().position(|t| t == "b").unwrap();
        doc.table_insert_column(b, false).unwrap();
        let t = doc.table_at(0).unwrap();
        assert_eq!(t.rows[0].cells[0].span, 3);
        assert_eq!(t.columns(), 3);
        let b = texts(&doc).iter().position(|t| t == "b").unwrap();
        doc.table_delete_column(b).unwrap();
        let t = doc.table_at(0).unwrap();
        assert_eq!(t.rows[0].cells[0].span, 2);
        assert_eq!(grid(&doc, 0), vec![vec!["wide"], vec!["a", ""]]);
    }

    #[test]
    fn widths_are_set_on_each_column_and_read_back() {
        let mut doc = parse("<table><tr><td>a</td><td width=\"100\">b</td></tr><tr><td>c</td><td>d</td></tr></table>", false);
        let t = doc.table_at(0).unwrap();
        assert_eq!(t.widths(), [None, Some(Width::Px(100.))]);
        assert_eq!(column_widths(&t.widths(), 500.), [400., 100.]);
        assert!(doc.set_table_widths(0, Some(&[30., 70.])));
        let t = doc.table_at(0).unwrap();
        assert_eq!(t.widths(), [Some(Width::Percent(30.)), Some(Width::Percent(70.))]);
        let html = to_html(&doc);
        assert!(html.contains("<td style=\"width:30%\">c</td>"), "{html}");
        assert!(!html.contains("width=\""), "{html}");
        let p = doc.table_percents(0).unwrap();
        assert!((p[0] - 30.).abs() < 0.01 && (p[1] - 70.).abs() < 0.01, "{p:?}");
        doc.set_table_widths(0, None);
        assert_eq!(doc.table_at(0).unwrap().widths(), [None, None]);
        assert_eq!(column_widths(&[None, None, None], 300.), [100., 100., 100.]);
        // Too wide together: scaled into the table.
        assert_eq!(column_widths(&[Some(Width::Px(400.)), Some(Width::Px(400.))], 400.), [200., 200.]);
    }

    #[test]
    fn enter_in_a_cell_stays_in_the_cell_and_tab_moves_between_cells() {
        let mut doc = parse("<table><tr><td>ab</td><td>c</td></tr></table>", false);
        let at = doc.split(Pos::new(0, 1));
        assert_eq!(at, Pos::new(1, 0));
        assert_eq!(grid(&doc, 0), [["a/b", "c"]]);
        assert!(same_cell(&doc.blocks[0].path, &doc.blocks[1].path));
        assert!(!same_cell(&doc.blocks[1].path, &doc.blocks[2].path));
        assert_eq!(doc.table_step(0, true), Some(Pos::new(2, 0)));
        assert_eq!(doc.table_step(2, false), Some(Pos::new(1, 1)));
        assert_eq!(doc.table_step(2, true), None);
        // Written back, one cell with its two lines.
        let html = to_html(&doc);
        assert!(html.contains("<td><div>a</div><div>b</div></td><td>c</td>"), "{html}");
        // Quoted, each cell's lines are quoted inside it.
        doc.toggle_quote(1..=2);
        assert_eq!(grid(&doc, 0), [["a/b", "c"]]);
        let html = to_html(&doc);
        assert!(html.contains("<td><div>a</div><blockquote><div>b</div></blockquote></td><td><blockquote><div>c</div></blockquote></td>"), "{html}");
        doc.toggle_quote(1..=2);
        assert!(to_html(&doc).contains("<td><div>a</div><div>b</div></td><td><div>c</div></td>"), "{}", to_html(&doc));
    }
}
