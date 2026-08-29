// mado-stats — pixel plugin for Mado
//
// Renders a Tasku statistics dashboard. Communicates via the Mado pixel
// plugin protocol: RGBA frames on stdout, JSON events on stdin.

use serde::Deserialize;
use std::io::{BufRead, BufReader, Write};
use std::process::Command;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

// ── Mado protocol ─────────────────────────────────────────────────────────────

fn send_frame(out: &mut impl Write, w: u32, h: u32, pixels: &[u8]) {
    let _ = out.write_all(b"MADO");
    let _ = out.write_all(&w.to_le_bytes());
    let _ = out.write_all(&h.to_le_bytes());
    let _ = out.write_all(pixels);
    let _ = out.flush();
}

#[derive(Deserialize)]
struct Event {
    #[serde(rename = "type")]
    kind: String,
    width:  Option<u32>,
    height: Option<u32>,
}

// ── Stats data ────────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
struct Stats {
    total:      u32,
    overdue:    u32,
    projects:   u32,
    todo:       u32,
    in_prog:    u32,
    done:       u32,
    cancelled:  u32,
    low:        u32,
    medium:     u32,
    high:       u32,
    urgent:     u32,
    by_project: Vec<(String, u32)>,
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // consume ESC [ ... <final-byte>  (final byte is in 0x40–0x7E)
            if chars.peek() == Some(&'[') { chars.next(); }
            for c in chars.by_ref() {
                if ('\x40'..='\x7e').contains(&c) { break; }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

fn fetch_stats() -> Option<Stats> {
    let out = Command::new("/bin/sh")
        .args(["-lc", "tasku stats"])
        .output()
        .ok()?;
    if !out.status.success() { return None; }
    let text = String::from_utf8_lossy(&out.stdout);
    parse_stats(&strip_ansi(&text))
}

fn parse_u32(line: &str) -> u32 {
    line.split_whitespace().last()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn parse_stats(text: &str) -> Option<Stats> {
    let mut s = Stats::default();
    let mut in_project = false;

    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() { in_project = false; continue; }

        if      t.starts_with("Total tasks:") { s.total     = parse_u32(t); }
        else if t.starts_with("Overdue:")     { s.overdue   = parse_u32(t); }
        else if t.starts_with("Projects:")    { s.projects  = parse_u32(t); }
        else if t.contains("Todo")            { s.todo      = parse_u32(t); }
        else if t.contains("In progress")     { s.in_prog   = parse_u32(t); }
        else if t.contains("Done")            { s.done      = parse_u32(t); }
        else if t.contains("Cancelled")       { s.cancelled = parse_u32(t); }
        else if t.contains("Low")             { s.low       = parse_u32(t); }
        else if t.contains("Medium")          { s.medium    = parse_u32(t); }
        else if t.contains("High")            { s.high      = parse_u32(t); }
        else if t.contains("Urgent")          { s.urgent    = parse_u32(t); }
        else if t.starts_with("By Project:")  { in_project  = true; }
        else if in_project {
            let parts: Vec<&str> = t.rsplitn(2, ' ').collect();
            if parts.len() == 2 {
                let count: u32 = parts[0].trim().parse().unwrap_or(0);
                let name = parts[1].trim().to_string();
                if !name.is_empty() {
                    s.by_project.push((name, count));
                }
            }
        }
    }
    Some(s)
}

// ── Renderer ──────────────────────────────────────────────────────────────────

const BG:     [u8; 4] = [15,  23,  42,  255]; // slate-900
const BORDER: [u8; 4] = [51,  65,  85,  255]; // slate-700
const TEXT:   [u8; 4] = [226, 232, 240, 255]; // slate-200
const MUTED:  [u8; 4] = [100, 116, 139, 255]; // slate-500
const GREEN:  [u8; 4] = [34,  197, 94,  255]; // green-500
const BLUE:   [u8; 4] = [96,  165, 250, 255]; // blue-400
const YELLOW: [u8; 4] = [234, 179, 8,   255]; // yellow-500
const RED:    [u8; 4] = [239, 68,  68,  255]; // red-500
const ORANGE: [u8; 4] = [249, 115, 22,  255]; // orange-500
const SLATE:  [u8; 4] = [71,  85,  105, 255]; // slate-600

struct Canvas {
    pixels: Vec<u8>,
    w: u32,
    h: u32,
}

impl Canvas {
    fn new(w: u32, h: u32) -> Self {
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h { pixels.extend_from_slice(&BG); }
        Self { pixels, w, h }
    }

    fn set(&mut self, x: i32, y: i32, color: [u8; 4]) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 { return; }
        let i = (y as u32 * self.w + x as u32) as usize * 4;
        self.pixels[i..i+4].copy_from_slice(&color);
    }

    fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, color: [u8; 4]) {
        for dy in 0..h { for dx in 0..w { self.set(x+dx, y+dy, color); } }
    }

    fn text(&mut self, x: i32, y: i32, s: &str, color: [u8; 4], scale: u32) {
        let mut cx = x;
        for ch in s.chars() {
            if let Some(glyph) = glyph(ch) {
                for (row, bits) in glyph.iter().enumerate() {
                    for col in 0..5usize {
                        if bits & (1 << (4 - col)) != 0 {
                            for sy in 0..scale as i32 {
                                for sx in 0..scale as i32 {
                                    self.set(cx + col as i32 * scale as i32 + sx,
                                             y  + row as i32 * scale as i32 + sy,
                                             color);
                                }
                            }
                        }
                    }
                }
            }
            cx += (6 * scale) as i32;
        }
    }

    fn bar(&mut self, x: i32, y: i32, w: i32, h: i32, frac: f32, color: [u8; 4]) {
        self.rect(x, y, w, h, SLATE);
        let filled = ((w as f32 * frac.clamp(0.0, 1.0)) as i32).max(0);
        if filled > 0 { self.rect(x, y, filled, h, color); }
    }

    fn text_w(s: &str, scale: u32) -> i32 {
        s.chars().count() as i32 * 6 * scale as i32
    }
}

// ── Dashboard layout ──────────────────────────────────────────────────────────

fn render(w: u32, h: u32, stats: &Stats) -> Vec<u8> {
    let mut c = Canvas::new(w, h);

    let pad = (w as i32 / 20).clamp(4, 20);
    let iw  = (w as i32 - pad * 2).max(1);
    if iw < 30 { return c.pixels; }

    // Text scale grows with available width
    let ts: u32 = if iw >= 900 { 4 } else if iw >= 500 { 3 } else if iw >= 200 { 2 } else { 1 };
    let lh = 7 * ts as i32 + 4;

    let mut y = pad;

    // Header
    let header = if iw >= 6 * ts as i32 * 10 { "TASK STATS" } else { "STATS" };
    c.text(pad, y, header, MUTED, ts);
    y += lh;
    c.rect(pad, y, iw, 1, BORDER);
    y += 6;

    // Summary row: 3 cols when wide enough
    if iw >= 90 {
        let col_w = iw / 3;
        let vs: u32 = if col_w >= 80 { 3 } else if col_w >= 48 { 2 } else { 1 };
        let ov_color = if stats.overdue > 0 { RED } else { GREEN };
        summary_cell(&mut c, pad,             y, col_w, &stats.total.to_string(),    "total",    TEXT,     vs, ts);
        summary_cell(&mut c, pad + col_w,     y, col_w, &stats.overdue.to_string(),  "overdue",  ov_color, vs, ts);
        summary_cell(&mut c, pad + col_w * 2, y, col_w, &stats.projects.to_string(), "projects", BLUE,     vs, ts);
        y += 7 * vs as i32 + lh + 6;
    } else {
        let line = format!("{} tasks  {} overdue", stats.total, stats.overdue);
        c.text(pad, y, &line, TEXT, ts);
        y += lh + 4;
    }

    if iw < 50 || y >= h as i32 { return c.pixels; }

    // By Status
    if y + lh < h as i32 { c.text(pad, y, "Status", MUTED, ts); y += lh; }
    let st = (stats.todo + stats.in_prog + stats.done + stats.cancelled).max(1);
    if y < h as i32 { y = stat_bar(&mut c, pad, y, iw, "Todo",        stats.todo,      st, BLUE,   ts); }
    if y < h as i32 { y = stat_bar(&mut c, pad, y, iw, "In Progress", stats.in_prog,   st, YELLOW, ts); }
    if y < h as i32 { y = stat_bar(&mut c, pad, y, iw, "Done",        stats.done,      st, GREEN,  ts); }
    if y < h as i32 { y = stat_bar(&mut c, pad, y, iw, "Cancelled",   stats.cancelled, st, SLATE,  ts); }
    y += 4;

    // By Priority
    if y + lh * 5 < h as i32 {
        if y + lh < h as i32 { c.text(pad, y, "Priority", MUTED, ts); y += lh; }
        let pt = (stats.low + stats.medium + stats.high + stats.urgent).max(1);
        if y < h as i32 { y = stat_bar(&mut c, pad, y, iw, "Low",    stats.low,    pt, SLATE,  ts); }
        if y < h as i32 { y = stat_bar(&mut c, pad, y, iw, "Medium", stats.medium, pt, BLUE,   ts); }
        if y < h as i32 { y = stat_bar(&mut c, pad, y, iw, "High",   stats.high,   pt, ORANGE, ts); }
        if y < h as i32 { y = stat_bar(&mut c, pad, y, iw, "Urgent", stats.urgent, pt, RED,    ts); }
        y += 4;
    }

    // By Project
    if !stats.by_project.is_empty() && y + lh * 2 < h as i32 {
        if y + lh < h as i32 { c.text(pad, y, "Projects", MUTED, ts); y += lh; }
        let pm = stats.by_project.iter().map(|(_, n)| *n).max().unwrap_or(1).max(1);
        for (name, count) in &stats.by_project {
            if y + lh > h as i32 { break; }
            y = stat_bar(&mut c, pad, y, iw, name, *count, pm, BLUE, ts);
        }
    }

    c.pixels
}

fn summary_cell(c: &mut Canvas, x: i32, y: i32, w: i32, val: &str, label: &str, color: [u8; 4], scale: u32, label_scale: u32) {
    let vw = Canvas::text_w(val, scale);
    c.text((x + (w - vw) / 2).max(x), y, val, color, scale);
    let lw = Canvas::text_w(label, label_scale);
    let ly = y + 7 * scale as i32 + 3;
    c.text((x + (w - lw) / 2).max(x), ly, label, MUTED, label_scale);
}

fn stat_bar(c: &mut Canvas, x: i32, y: i32, w: i32, label: &str, count: u32, total: u32, color: [u8; 4], ts: u32) -> i32 {
    let char_w = 6 * ts as i32;
    let text_h = 7 * ts as i32;

    // Bar is a short indicator, not the focus
    let bar_w   = (w / 15).max(4);
    let count_w = char_w * 4;
    let label_w = (w - bar_w - count_w - 8).max(char_w);

    let max_chars = (label_w / char_w).max(1) as usize;
    let truncated: String = label.chars().take(max_chars).collect();
    c.text(x, y, &truncated, TEXT, ts);

    // Bar vertically centred with the text
    let bar_h = (text_h / 2).max(3);
    let bar_y  = y + (text_h - bar_h) / 2;
    c.bar(x + label_w + 4, bar_y, bar_w, bar_h, count as f32 / total as f32, color);

    // Count right-aligned
    let cs  = count.to_string();
    let cs_w = Canvas::text_w(&cs, ts);
    c.text(x + w - cs_w, y, &cs, MUTED, ts);

    y + text_h + 4
}

// ── Minimal 5×7 bitmap font ───────────────────────────────────────────────────

fn glyph(ch: char) -> Option<[u8; 7]> {
    Some(match ch {
        ' ' => [0x00,0x00,0x00,0x00,0x00,0x00,0x00],
        'A' => [0x0E,0x11,0x11,0x1F,0x11,0x11,0x11],
        'B' => [0x1E,0x11,0x11,0x1E,0x11,0x11,0x1E],
        'C' => [0x0E,0x11,0x10,0x10,0x10,0x11,0x0E],
        'D' => [0x1E,0x09,0x09,0x09,0x09,0x09,0x1E],
        'E' => [0x1F,0x10,0x10,0x1E,0x10,0x10,0x1F],
        'F' => [0x1F,0x10,0x10,0x1E,0x10,0x10,0x10],
        'G' => [0x0E,0x11,0x10,0x17,0x11,0x11,0x0F],
        'H' => [0x11,0x11,0x11,0x1F,0x11,0x11,0x11],
        'I' => [0x0E,0x04,0x04,0x04,0x04,0x04,0x0E],
        'J' => [0x07,0x02,0x02,0x02,0x02,0x12,0x0C],
        'K' => [0x11,0x12,0x14,0x18,0x14,0x12,0x11],
        'L' => [0x10,0x10,0x10,0x10,0x10,0x10,0x1F],
        'M' => [0x11,0x1B,0x15,0x11,0x11,0x11,0x11],
        'N' => [0x11,0x19,0x15,0x13,0x11,0x11,0x11],
        'O' => [0x0E,0x11,0x11,0x11,0x11,0x11,0x0E],
        'P' => [0x1E,0x11,0x11,0x1E,0x10,0x10,0x10],
        'Q' => [0x0E,0x11,0x11,0x11,0x15,0x12,0x0D],
        'R' => [0x1E,0x11,0x11,0x1E,0x14,0x12,0x11],
        'S' => [0x0F,0x10,0x10,0x0E,0x01,0x01,0x1E],
        'T' => [0x1F,0x04,0x04,0x04,0x04,0x04,0x04],
        'U' => [0x11,0x11,0x11,0x11,0x11,0x11,0x0E],
        'V' => [0x11,0x11,0x11,0x11,0x11,0x0A,0x04],
        'W' => [0x11,0x11,0x11,0x15,0x15,0x1B,0x11],
        'X' => [0x11,0x11,0x0A,0x04,0x0A,0x11,0x11],
        'Y' => [0x11,0x11,0x0A,0x04,0x04,0x04,0x04],
        'Z' => [0x1F,0x01,0x02,0x04,0x08,0x10,0x1F],
        'a' => [0x00,0x00,0x0E,0x01,0x0F,0x11,0x0F],
        'b' => [0x10,0x10,0x1E,0x11,0x11,0x11,0x1E],
        'c' => [0x00,0x00,0x0E,0x10,0x10,0x11,0x0E],
        'd' => [0x01,0x01,0x0F,0x11,0x11,0x11,0x0F],
        'e' => [0x00,0x00,0x0E,0x11,0x1F,0x10,0x0E],
        'f' => [0x06,0x09,0x08,0x1C,0x08,0x08,0x08],
        'g' => [0x00,0x0F,0x11,0x11,0x0F,0x01,0x0E],
        'h' => [0x10,0x10,0x16,0x19,0x11,0x11,0x11],
        'i' => [0x04,0x00,0x0C,0x04,0x04,0x04,0x0E],
        'j' => [0x02,0x00,0x06,0x02,0x02,0x12,0x0C],
        'k' => [0x10,0x10,0x12,0x14,0x18,0x14,0x12],
        'l' => [0x0C,0x04,0x04,0x04,0x04,0x04,0x0E],
        'm' => [0x00,0x00,0x1A,0x15,0x15,0x11,0x11],
        'n' => [0x00,0x00,0x16,0x19,0x11,0x11,0x11],
        'o' => [0x00,0x00,0x0E,0x11,0x11,0x11,0x0E],
        'p' => [0x00,0x1E,0x11,0x11,0x1E,0x10,0x10],
        'q' => [0x00,0x0F,0x11,0x11,0x0F,0x01,0x01],
        'r' => [0x00,0x00,0x16,0x19,0x10,0x10,0x10],
        's' => [0x00,0x00,0x0E,0x10,0x0E,0x01,0x1E],
        't' => [0x08,0x08,0x1C,0x08,0x08,0x09,0x06],
        'u' => [0x00,0x00,0x11,0x11,0x11,0x13,0x0D],
        'v' => [0x00,0x00,0x11,0x11,0x11,0x0A,0x04],
        'w' => [0x00,0x00,0x11,0x15,0x15,0x15,0x0A],
        'x' => [0x00,0x00,0x11,0x0A,0x04,0x0A,0x11],
        'y' => [0x00,0x11,0x11,0x0F,0x01,0x11,0x0E],
        'z' => [0x00,0x00,0x1F,0x02,0x04,0x08,0x1F],
        '0' => [0x0E,0x11,0x13,0x15,0x19,0x11,0x0E],
        '1' => [0x04,0x0C,0x04,0x04,0x04,0x04,0x0E],
        '2' => [0x0E,0x11,0x01,0x06,0x08,0x10,0x1F],
        '3' => [0x1F,0x02,0x04,0x06,0x01,0x11,0x0E],
        '4' => [0x02,0x06,0x0A,0x12,0x1F,0x02,0x02],
        '5' => [0x1F,0x10,0x1E,0x01,0x01,0x11,0x0E],
        '6' => [0x06,0x08,0x10,0x1E,0x11,0x11,0x0E],
        '7' => [0x1F,0x01,0x02,0x04,0x08,0x08,0x08],
        '8' => [0x0E,0x11,0x11,0x0E,0x11,0x11,0x0E],
        '9' => [0x0E,0x11,0x11,0x0F,0x01,0x02,0x0C],
        ':' => [0x00,0x04,0x00,0x00,0x04,0x00,0x00],
        '-' => [0x00,0x00,0x00,0x1F,0x00,0x00,0x00],
        '/' => [0x01,0x02,0x02,0x04,0x08,0x08,0x10],
        '.' => [0x00,0x00,0x00,0x00,0x00,0x00,0x04],
        '%' => [0x19,0x1A,0x02,0x04,0x08,0x0B,0x13],
        _   => [0x00,0x00,0x00,0x00,0x00,0x00,0x00],
    })
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());

    let size:  Arc<Mutex<(u32, u32)>> = Arc::new(Mutex::new((300, 400)));
    let stats: Arc<Mutex<Stats>>      = Arc::new(Mutex::new(Stats::default()));

    // Channel: signals the render loop to redraw immediately (e.g. on resize).
    let (tx, rx) = mpsc::channel::<()>();

    // Stdin reader — handles resize events, triggers immediate re-render
    {
        let size = Arc::clone(&size);
        let tx   = tx.clone();
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            for line in BufReader::new(stdin.lock()).lines() {
                let Ok(line) = line else { break };
                if let Ok(ev) = serde_json::from_str::<Event>(&line) {
                    if ev.kind == "resize" {
                        if let (Some(w), Some(h)) = (ev.width, ev.height) {
                            *size.lock().unwrap() = (w, h);
                            let _ = tx.send(());  // wake render loop immediately
                        }
                    }
                }
            }
        });
    }

    // Initial fetch
    if let Some(s) = fetch_stats() { *stats.lock().unwrap() = s; }
    let mut last_fetch = Instant::now();

    // Render once immediately
    let _ = tx.send(());

    loop {
        // Wait for a resize signal or timeout after 60s for data refresh
        let _ = rx.recv_timeout(Duration::from_secs(60));
        // Drain any extra signals that arrived while we were rendering
        while rx.try_recv().is_ok() {}

        // Refresh stats if 60s have passed
        if last_fetch.elapsed() >= Duration::from_secs(60) {
            if let Some(s) = fetch_stats() { *stats.lock().unwrap() = s; }
            last_fetch = Instant::now();
        }

        let (w, h) = *size.lock().unwrap();
        let pixels  = render(w, h, &stats.lock().unwrap().clone());
        send_frame(&mut out, w, h, &pixels);
    }
}
