//! Pure functions turning the document into Slint view models.

use crate::{Bar, Block, BoardCol, CellItem, DayCell, DayHead, Section, TaskItem};
use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime};
use patch_core::model::{Column, List, Repeat, Task};
use patch_core::recur::occurrences;
use patch_core::time::{fmt_time, month_grid, week_start};
use slint::{Color, ModelRc, SharedString, VecModel};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

pub fn color(hex: &str) -> Color {
    let h = hex.trim_start_matches('#');
    match (h.len(), u32::from_str_radix(h, 16)) {
        (6, Ok(v)) => Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8),
        (8, Ok(v)) => Color::from_argb_u8(v as u8, (v >> 24) as u8, (v >> 16) as u8, (v >> 8) as u8),
        _ => Color::from_rgb_u8(0x0a, 0x84, 0xff),
    }
}

pub fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

/// Everything the builders need, computed once per refresh.
pub struct Ctx {
    pub tasks: Vec<Task>,
    pub lists: HashMap<String, List>,
    pub hidden: HashSet<String>,
    pub now: NaiveDateTime,
    pub h24: bool,
    pub monday: bool,
}

impl Ctx {
    pub fn today(&self) -> NaiveDate {
        self.now.date()
    }

    pub fn visible(&self) -> impl Iterator<Item = &Task> {
        self.tasks.iter().filter(|t| !self.hidden.contains(&t.list))
    }

    fn list_color(&self, t: &Task) -> Color {
        self.lists.get(&t.list).map(|l| color(&l.color)).unwrap_or_else(|| color("#0A84FF"))
    }

    fn list_name(&self, t: &Task) -> String {
        self.lists.get(&t.list).map(|l| l.name.clone()).unwrap_or_default()
    }

    pub fn day_name(&self, d: NaiveDate) -> String {
        let today = self.today();
        match (d - today).num_days() {
            0 => "Today".into(),
            1 => "Tomorrow".into(),
            -1 => "Yesterday".into(),
            2..=6 => d.format("%A").to_string(),
            _ if d.year() == today.year() => d.format("%a %-d %b").to_string(),
            _ => d.format("%-d %b %Y").to_string(),
        }
    }

    pub fn time(&self, dt: NaiveDateTime) -> String {
        fmt_time(dt, self.h24)
    }

    /// "Today, 10:00", "13 – 15 Oct", "Friday".
    pub fn when(&self, t: &Task, start: NaiveDateTime) -> String {
        if t.all_day {
            let last = t.end.map(|e| e.date() + (start.date() - t.start.map(|s| s.date()).unwrap_or(start.date())));
            match last.filter(|l| *l > start.date()) {
                Some(l) if l.month() == start.month() && (start.date() - self.today()).num_days().abs() > 6 => {
                    format!("{} – {}", start.format("%-d"), l.format("%-d %b"))
                }
                Some(l) => format!("{} – {}", self.day_name(start.date()), self.day_name(l)),
                None => self.day_name(start.date()),
            }
        } else {
            format!("{}, {}", self.day_name(start.date()), self.time(start))
        }
    }

    pub fn task_item(&self, t: &Task, start: Option<NaiveDateTime>) -> TaskItem {
        let start = start.or(t.start);
        let (done_items, total) = t.checklist_progress();
        let today = self.today();
        let is_today = start.is_some_and(|s| {
            let end = s + t.duration();
            s.date() <= today && (end - Duration::minutes(1)).date() >= today
        });
        TaskItem {
            id: t.id.clone().into(),
            title: t.title.clone().into(),
            subtitle: SharedString::default(),
            list_name: self.list_name(t).into(),
            color: self.list_color(t),
            done: t.done,
            priority: t.priority as i32,
            overdue: t.is_overdue(self.now),
            when: start.map(|s| self.when(t, s)).unwrap_or_default().into(),
            has_notes: !t.notes.trim().is_empty(),
            check_done: done_items as i32,
            check_total: total as i32,
            repeat: t.repeat != Repeat::None,
            today: is_today && !t.done,
        }
    }
}

/// Packs spans (inclusive column ranges) into the fewest rows; input must be
/// sorted by start then by descending length.
fn assign_lanes(spans: &[(usize, usize)], width: usize) -> Vec<usize> {
    let mut rows: Vec<Vec<bool>> = Vec::new();
    spans
        .iter()
        .map(|&(a, b)| {
            let lane = rows.iter().position(|r| (a..=b).all(|c| !r[c])).unwrap_or_else(|| {
                rows.push(vec![false; width]);
                rows.len() - 1
            });
            rows[lane][a..=b].fill(true);
            lane
        })
        .collect()
}

fn midnight(d: NaiveDate) -> NaiveDateTime {
    d.and_time(NaiveTime::MIN)
}

/// Inclusive last day an occurrence touches.
fn last_day(end: NaiveDateTime) -> NaiveDate {
    (end - Duration::minutes(1)).date()
}

struct Span {
    first: usize,
    last: usize,
    starts_before: bool,
    ends_after: bool,
    task: usize,
}

/// macOS-style month: multi-day items become bars sharing a lane across the
/// week; single-day timed items are dots below them.
pub fn month_cells(ctx: &Ctx, anchor: NaiveDate, selected: NaiveDate, max_items: usize) -> Vec<DayCell> {
    let grid = month_grid(anchor, ctx.monday);
    let month = anchor.month();
    let today = ctx.today();
    let visible: Vec<&Task> = ctx.visible().filter(|t| t.start.is_some()).collect();
    let mut cells = Vec::with_capacity(42);

    for week in grid.chunks(7) {
        let (w0, w_last) = (week[0], week[6]);
        let mut spans: Vec<Span> = Vec::new();
        let mut dots: Vec<Vec<(NaiveDateTime, usize)>> = vec![Vec::new(); 7];
        for (ti, t) in visible.iter().enumerate() {
            for occ in occurrences(t, midnight(w0), midnight(w_last + Duration::days(1))) {
                let (s, e) = (occ.start.date(), last_day(occ.end));
                if t.all_day || e > s {
                    let first = (s.max(w0) - w0).num_days() as usize;
                    let last = (e.min(w_last) - w0).num_days() as usize;
                    spans.push(Span { first, last, starts_before: s < w0, ends_after: e > w_last, task: ti });
                } else {
                    dots[(s - w0).num_days() as usize].push((occ.start, ti));
                }
            }
        }
        spans.sort_by_key(|s| (s.first, usize::MAX - (s.last - s.first)));
        let lanes = assign_lanes(&spans.iter().map(|s| (s.first, s.last)).collect::<Vec<_>>(), 7);

        for (di, day) in week.iter().enumerate() {
            let mut items: Vec<CellItem> = Vec::new();
            let mut colors: Vec<Color> = Vec::new();
            for (span, &lane) in spans.iter().zip(&lanes) {
                if di < span.first || di > span.last {
                    continue;
                }
                let t = visible[span.task];
                while items.len() <= lane {
                    items.push(CellItem::default());
                }
                let c = ctx.list_color(t);
                colors.push(c);
                items[lane] = CellItem {
                    id: t.id.clone().into(),
                    title: if di == span.first || di == 0 { t.title.clone().into() } else { SharedString::default() },
                    time: SharedString::default(),
                    color: c,
                    kind: 1,
                    cont_left: di > span.first || span.starts_before,
                    cont_right: di < span.last || span.ends_after,
                    done: t.done,
                };
            }
            let mut day_dots = dots[di].clone();
            day_dots.sort_by_key(|(s, _)| *s);
            for (start, ti) in day_dots {
                let t = visible[ti];
                let c = ctx.list_color(t);
                colors.push(c);
                items.push(CellItem {
                    id: t.id.clone().into(),
                    title: t.title.clone().into(),
                    time: ctx.time(start).into(),
                    color: c,
                    kind: 2,
                    cont_left: false,
                    cont_right: false,
                    done: t.done,
                });
            }
            let more = items.len().saturating_sub(max_items);
            items.truncate(max_items);
            colors.dedup();
            colors.truncate(3);
            cells.push(DayCell {
                key: day.to_string().into(),
                day: day.day() as i32,
                label: if day.day() == 1 { day.format("%b %-d").to_string() } else { day.day().to_string() }.into(),
                in_month: day.month() == month,
                today: *day == today,
                selected: *day == selected,
                weekend: matches!(day.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun),
                range_start: false,
                range_end: false,
                in_range: false,
                dots: model(colors),
                items: model(items),
                more: more as i32,
            });
        }
    }
    cells
}

/// Sidebar month: just numbers and a dot for busy days.
pub fn mini_cells(ctx: &Ctx, month: NaiveDate, selected: NaiveDate) -> Vec<DayCell> {
    let grid = month_grid(month, ctx.monday);
    let from = midnight(grid[0]);
    let to = midnight(grid[41] + Duration::days(1));
    let mut busy: HashMap<NaiveDate, Color> = HashMap::new();
    for t in ctx.visible().filter(|t| !t.done) {
        for occ in occurrences(t, from, to) {
            let mut d = occ.start.date().max(grid[0]);
            while d <= last_day(occ.end).min(grid[41]) {
                busy.entry(d).or_insert_with(|| ctx.list_color(t));
                d += Duration::days(1);
            }
        }
    }
    grid.iter()
        .map(|d| DayCell {
            key: d.to_string().into(),
            day: d.day() as i32,
            label: d.day().to_string().into(),
            in_month: d.month() == month.month(),
            today: *d == ctx.today(),
            selected: *d == selected,
            weekend: matches!(d.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun),
            dots: model(busy.get(d).map(|c| vec![*c]).unwrap_or_default()),
            ..Default::default()
        })
        .collect()
}

/// The range picker grid: only the needed weeks of `month`.
pub fn picker_cells(month: NaiveDate, start: Option<NaiveDate>, end: Option<NaiveDate>, monday: bool, today: NaiveDate) -> Vec<DayCell> {
    let grid = month_grid(month, monday);
    let last_needed = grid.iter().rposition(|d| d.month() == month.month()).unwrap_or(41);
    let weeks = last_needed / 7 + 1;
    let end = end.or(start);
    grid.into_iter()
        .take(weeks * 7)
        .map(|d| {
            let (rs, re) = (start == Some(d), end == Some(d));
            let inside = matches!((start, end), (Some(s), Some(e)) if d > s && d < e);
            DayCell {
                key: d.to_string().into(),
                day: d.day() as i32,
                label: d.day().to_string().into(),
                in_month: d.month() == month.month(),
                today: d == today,
                range_start: rs,
                range_end: re,
                in_range: inside,
                ..Default::default()
            }
        })
        .collect()
}

pub struct Grid {
    pub days: Vec<DayHead>,
    pub blocks: Vec<Block>,
    pub bars: Vec<Bar>,
    pub lanes: i32,
}

/// Week/day time grid with overlapping blocks laid out side by side.
pub fn time_grid(ctx: &Ctx, days: &[NaiveDate]) -> Grid {
    let first = days[0];
    let last = *days.last().unwrap_or(&first);
    let n = days.len();
    let heads = days
        .iter()
        .map(|d| DayHead {
            key: d.to_string().into(),
            weekday: d.format("%a").to_string().into(),
            day: d.day() as i32,
            today: *d == ctx.today(),
            weekend: matches!(d.weekday(), chrono::Weekday::Sat | chrono::Weekday::Sun),
        })
        .collect();

    // (first col, last col, starts before, ends after, task)
    type AllDay<'a> = (usize, usize, bool, bool, &'a Task);
    // (top minute, bottom minute, task, occurrence start, occurrence end)
    type Timed<'a> = (i64, i64, &'a Task, NaiveDateTime, NaiveDateTime);
    let mut spans: Vec<AllDay> = Vec::new();
    let mut per_day: Vec<Vec<Timed>> = vec![Vec::new(); n];

    for t in ctx.visible().filter(|t| t.start.is_some()) {
        for occ in occurrences(t, midnight(first), midnight(last + Duration::days(1))) {
            if t.all_day {
                let (s, e) = (occ.start.date(), last_day(occ.end));
                let a = (s.max(first) - first).num_days() as usize;
                let b = (e.min(last) - first).num_days() as usize;
                spans.push((a, b, s < first, e > last, t));
                continue;
            }
            for (di, day) in days.iter().enumerate() {
                let day_start = midnight(*day);
                let day_end = day_start + Duration::days(1);
                if occ.end <= day_start || occ.start >= day_end {
                    continue;
                }
                let top = (occ.start.max(day_start) - day_start).num_minutes();
                let bottom = (occ.end.min(day_end) - day_start).num_minutes();
                per_day[di].push((top, bottom.max(top + 15), t, occ.start, occ.end));
            }
        }
    }

    spans.sort_by_key(|s| (s.0, usize::MAX - (s.1 - s.0)));
    let lanes = assign_lanes(&spans.iter().map(|s| (s.0, s.1)).collect::<Vec<_>>(), n);
    let bars = spans
        .iter()
        .zip(&lanes)
        .map(|(&(a, b, before, after, t), &lane)| Bar {
            id: t.id.clone().into(),
            title: t.title.clone().into(),
            color: ctx.list_color(t),
            col: a as i32,
            span: (b - a + 1) as i32,
            lane: lane as i32,
            done: t.done,
            cont_left: before,
            cont_right: after,
        })
        .collect();

    let mut blocks = Vec::new();
    for (di, mut items) in per_day.into_iter().enumerate() {
        items.sort_by_key(|i| (i.0, -(i.1)));
        // Clusters of transitively overlapping items share a column count.
        let mut i = 0;
        while i < items.len() {
            let mut j = i;
            let mut cluster_end = items[i].1;
            while j + 1 < items.len() && items[j + 1].0 < cluster_end {
                j += 1;
                cluster_end = cluster_end.max(items[j].1);
            }
            let cluster = &items[i..=j];
            let mut col_ends: Vec<i64> = Vec::new();
            let cols: Vec<usize> = cluster
                .iter()
                .map(|it| match col_ends.iter().position(|&e| e <= it.0) {
                    Some(c) => {
                        col_ends[c] = it.1;
                        c
                    }
                    None => {
                        col_ends.push(it.1);
                        col_ends.len() - 1
                    }
                })
                .collect();
            let ncols = col_ends.len().max(1) as f32;
            for (it, c) in cluster.iter().zip(cols) {
                let (top, bottom, t, s, e) = *it;
                blocks.push(Block {
                    id: t.id.clone().into(),
                    title: t.title.clone().into(),
                    time: format!("{} – {}", ctx.time(s), ctx.time(e)).into(),
                    color: ctx.list_color(t),
                    day: di as i32,
                    lane_x: c as f32 / ncols,
                    lane_w: 1.0 / ncols,
                    top: top as f32,
                    span: (bottom - top) as f32,
                    done: t.done,
                });
            }
            i = j + 1;
        }
    }

    Grid { days: heads, blocks, bars, lanes: lanes.iter().map(|l| *l as i32 + 1).max().unwrap_or(0) }
}

pub fn board(ctx: &Ctx, columns: &[Column]) -> Vec<BoardCol> {
    columns
        .iter()
        .map(|c| {
            let mut cards: Vec<&Task> = ctx.visible().filter(|t| t.column == c.id).collect();
            if c.done {
                // Most recently finished first; keep the column manageable.
                cards.sort_by_key(|t| std::cmp::Reverse(t.done_at.unwrap_or(0)));
                cards.truncate(60);
            }
            BoardCol {
                id: c.id.clone().into(),
                name: c.name.clone().into(),
                done: c.done,
                count: cards.len() as i32,
                cards: model(cards.iter().map(|t| ctx.task_item(t, None)).collect()),
            }
        })
        .collect()
}

/// Groups for the list view.
pub fn sections(ctx: &Ctx, tint: (Color, Color, Color)) -> Vec<Section> {
    let (danger, accent, plain) = tint;
    let today = ctx.today();
    let mut overdue = Vec::new();
    let mut today_v = Vec::new();
    let mut tomorrow = Vec::new();
    let mut week = Vec::new();
    let mut later = Vec::new();
    let mut someday = Vec::new();
    let mut done: Vec<&Task> = Vec::new();

    for t in ctx.visible() {
        if t.done {
            done.push(t);
            continue;
        }
        let Some(start) = t.start else {
            someday.push(t);
            continue;
        };
        let last = last_day(t.span_end().unwrap_or(start));
        if t.is_overdue(ctx.now) {
            overdue.push(t);
        } else if start.date() <= today && last >= today {
            today_v.push(t);
        } else if start.date() == today + Duration::days(1) {
            tomorrow.push(t);
        } else if start.date() <= today + Duration::days(7) {
            week.push(t);
        } else {
            later.push(t);
        }
    }
    let by_start = |v: &mut Vec<&Task>| v.sort_by_key(|t| (t.start, !t.all_day, std::cmp::Reverse(t.priority)));
    for v in [&mut overdue, &mut today_v, &mut tomorrow, &mut week, &mut later] {
        by_start(v);
    }
    someday.sort_by_key(|t| (std::cmp::Reverse(t.priority), (t.order * 1000.0) as i64));
    done.sort_by_key(|t| std::cmp::Reverse(t.done_at.unwrap_or(0)));
    done.truncate(30);

    let mut out = Vec::new();
    let mut push = |title: &str, tint: Color, v: Vec<&Task>| {
        if !v.is_empty() {
            out.push(Section {
                title: title.into(),
                tint,
                items: model(v.iter().map(|t| ctx.task_item(t, None)).collect()),
            });
        }
    };
    push("Overdue", danger, overdue);
    push("Today", accent, today_v);
    push("Tomorrow", plain, tomorrow);
    push("Next 7 days", plain, week);
    push("Later", plain, later);
    push("Someday", plain, someday);
    push("Completed", plain, done);
    out
}

/// Everything happening on `day`, all-day first, then by time.
pub fn agenda(ctx: &Ctx, day: NaiveDate) -> Vec<TaskItem> {
    let mut v: Vec<(bool, NaiveDateTime, TaskItem)> = Vec::new();
    for t in ctx.visible() {
        for occ in occurrences(t, midnight(day), midnight(day + Duration::days(1))) {
            v.push((!t.all_day, occ.start, ctx.task_item(t, Some(occ.start))));
        }
    }
    v.sort_by_key(|x| (x.0, x.1));
    v.into_iter().map(|x| x.2).collect()
}

/// Overdue and today's open tasks, then what was finished today.
pub fn today_items(ctx: &Ctx) -> (Vec<TaskItem>, usize) {
    let today = ctx.today();
    let mut open: Vec<&Task> = ctx
        .visible()
        .filter(|t| !t.done)
        .filter(|t| t.start.is_some_and(|s| s.date() <= today))
        .filter(|t| t.is_overdue(ctx.now) || t.span_end().is_some_and(|e| last_day(e) >= today))
        .collect();
    open.sort_by_key(|t| (!t.is_overdue(ctx.now), t.all_day, t.start));
    let count = open.len();
    let finished_today = ctx.visible().filter(|t| {
        t.done
            && t.done_at.is_some_and(|ms| {
                chrono::DateTime::from_timestamp_millis(ms)
                    .map(|d| d.with_timezone(&chrono::Local).date_naive() == today)
                    .unwrap_or(false)
            })
    });
    let items = open.into_iter().chain(finished_today).map(|t| ctx.task_item(t, None)).collect();
    (items, count)
}

pub fn up_next(ctx: &Ctx, n: usize) -> Vec<TaskItem> {
    let mut v: Vec<(NaiveDateTime, &Task)> = ctx
        .visible()
        .filter(|t| !t.done)
        .filter_map(|t| t.start.map(|s| (s, t)))
        .filter(|(_, t)| t.span_end().is_some_and(|e| e > ctx.now))
        .filter(|(s, _)| *s <= ctx.now + Duration::days(14))
        .collect();
    v.sort_by_key(|(s, _)| *s);
    v.into_iter().take(n).map(|(s, t)| ctx.task_item(t, Some(s))).collect()
}

pub fn search(ctx: &Ctx, query: &str) -> Vec<TaskItem> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<&Task> = ctx
        .tasks
        .iter()
        .filter(|t| t.title.to_lowercase().contains(&q) || t.notes.to_lowercase().contains(&q))
        .collect();
    hits.sort_by_key(|t| (t.done, !t.title.to_lowercase().starts_with(&q), t.start));
    hits.into_iter().take(40).map(|t| ctx.task_item(t, None)).collect()
}

pub fn week_days(anchor: NaiveDate, monday: bool) -> Vec<NaiveDate> {
    let s = week_start(anchor, monday);
    (0..7).map(|i| s + Duration::days(i)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use patch_core::time::parse_dt;

    fn ctx(tasks: Vec<Task>) -> Ctx {
        Ctx {
            tasks,
            lists: HashMap::new(),
            hidden: HashSet::new(),
            now: parse_dt("2026-10-22T09:30").unwrap(),
            h24: true,
            monday: true,
        }
    }

    fn task(id: &str, start: &str, end: Option<&str>, all_day: bool) -> Task {
        let mut t = Task::blank(id, "l", "c");
        t.title = id.into();
        t.start = parse_dt(start);
        t.end = end.and_then(parse_dt);
        t.all_day = all_day;
        t
    }

    #[test]
    fn lanes_pack_tightly() {
        assert_eq!(assign_lanes(&[(0, 2), (1, 1), (3, 4), (2, 3)], 7), vec![0, 1, 0, 1]);
    }

    #[test]
    fn multi_day_bars_keep_their_lane_across_the_week() {
        let c = ctx(vec![
            task("trip", "2026-10-13", Some("2026-10-15"), true),
            task("call", "2026-10-14T10:00", None, false),
        ]);
        let anchor = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        let cells = month_cells(&c, anchor, anchor, 6);
        let by_key = |k: &str| cells.iter().find(|c| c.key == k).unwrap().clone();
        let tue = by_key("2026-10-13");
        let wed = by_key("2026-10-14");
        use slint::Model;
        let t0 = tue.items.row_data(0).unwrap();
        assert_eq!((t0.kind, t0.cont_left, t0.cont_right), (1, false, true));
        assert_eq!(t0.title, "trip");
        let w0 = wed.items.row_data(0).unwrap();
        assert_eq!((w0.kind, w0.cont_left, w0.cont_right, w0.title.as_str()), (1, true, true, ""));
        let w1 = wed.items.row_data(1).unwrap();
        assert_eq!((w1.kind, w1.title.as_str(), w1.time.as_str()), (2, "call", "10:00"));
    }

    #[test]
    fn overlapping_blocks_split_columns() {
        let c = ctx(vec![
            task("a", "2026-10-22T09:00", Some("2026-10-22T11:00"), false),
            task("b", "2026-10-22T10:00", Some("2026-10-22T10:30"), false),
            task("c", "2026-10-22T12:00", Some("2026-10-22T13:00"), false),
        ]);
        let day = NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
        let g = time_grid(&c, &[day]);
        let get = |id: &str| g.blocks.iter().find(|b| b.id == id).unwrap().clone();
        assert_eq!((get("a").lane_x, get("a").lane_w), (0.0, 0.5));
        assert_eq!((get("b").lane_x, get("b").lane_w), (0.5, 0.5));
        assert_eq!((get("c").lane_x, get("c").lane_w), (0.0, 1.0));
        assert_eq!(get("a").top, 540.0);
        assert_eq!(get("a").span, 120.0);
    }

    #[test]
    fn overnight_block_is_clipped_per_day() {
        let c = ctx(vec![task("night", "2026-10-22T22:00", Some("2026-10-23T02:00"), false)]);
        let days = week_days(NaiveDate::from_ymd_opt(2026, 10, 22).unwrap(), true);
        let g = time_grid(&c, &days);
        let spans: Vec<(i32, f32, f32)> = g.blocks.iter().map(|b| (b.day, b.top, b.span)).collect();
        assert_eq!(spans, vec![(3, 1320.0, 120.0), (4, 0.0, 120.0)]);
    }

    #[test]
    fn sections_group_by_due() {
        let mut done = task("done", "2026-10-20T09:00", None, false);
        done.done = true;
        let c = ctx(vec![
            task("late", "2026-10-21T09:00", None, false),
            task("now", "2026-10-22T15:00", None, false),
            task("tmr", "2026-10-23", None, true),
            Task { title: "someday".into(), ..Task::blank("s", "l", "c") },
            done,
        ]);
        let titles: Vec<String> = sections(&c, (Color::default(), Color::default(), Color::default()))
            .iter()
            .map(|s| s.title.to_string())
            .collect();
        assert_eq!(titles, vec!["Overdue", "Today", "Tomorrow", "Someday", "Completed"]);
    }

    #[test]
    fn picker_marks_range() {
        let m = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap();
        let s = NaiveDate::from_ymd_opt(2026, 10, 13);
        let e = NaiveDate::from_ymd_opt(2026, 10, 15);
        let cells = picker_cells(m, s, e, true, m);
        assert_eq!(cells.len(), 35);
        let f = |d: &str| cells.iter().find(|c| c.key == d).unwrap().clone();
        assert!(f("2026-10-13").range_start && f("2026-10-15").range_end && f("2026-10-14").in_range);
        assert!(!f("2026-10-16").in_range);
    }

    #[test]
    fn when_labels() {
        let c = ctx(vec![]);
        let t = task("x", "2026-10-22T10:00", None, false);
        assert_eq!(c.when(&t, t.start.unwrap()), "Today, 10:00");
        let t = task("y", "2026-11-13", Some("2026-11-15"), true);
        assert_eq!(c.when(&t, t.start.unwrap()), "13 – 15 Nov");
    }
}
