/// Interactive TUI — CPU (per-core gauges + history curve), Memory, GPU, processes.
/// Run with: `deepspeed monitor`
use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{
        Axis, Block, Borders, Cell, Chart, Dataset, Gauge, GraphType, Paragraph, Row, Table,
        TableState,
    },
    Frame, Terminal,
};

use crate::config::Config;
use crate::monitor::{MemoryPressureLevel, SystemMonitor, SystemSnapshot, ThermalState};

const MAX_HISTORY: usize = 60; // number of samples kept (60 × 2 s = 2 min)
const MAX_LOG: usize = 200;
const REFRESH_SECS: u64 = 2;

// ── App state ──────────────────────────────────────────────────────────────

struct App {
    snapshot: Option<SystemSnapshot>,
    log: VecDeque<String>,
    process_scroll: usize,
    last_refresh: Instant,
    config: Config,
    // Rolling history for 2-D curves
    cpu_history: VecDeque<f64>,
    mem_history: VecDeque<f64>,
    swap_history: VecDeque<f64>,
    gpu_history: VecDeque<f64>,
}

impl App {
    fn new(config: Config) -> Self {
        let refresh = Duration::from_secs(REFRESH_SECS);
        Self {
            snapshot: None,
            log: VecDeque::with_capacity(MAX_LOG),
            process_scroll: 0,
            last_refresh: Instant::now() - refresh - Duration::from_millis(1),
            config,
            cpu_history: VecDeque::with_capacity(MAX_HISTORY),
            mem_history: VecDeque::with_capacity(MAX_HISTORY),
            swap_history: VecDeque::with_capacity(MAX_HISTORY),
            gpu_history: VecDeque::with_capacity(MAX_HISTORY),
        }
    }

    fn ingest_snapshot(&mut self, snap: SystemSnapshot) {
        push_history(&mut self.cpu_history, snap.cpu.usage_pct);
        push_history(&mut self.mem_history, snap.memory.used_pct);
        push_history(&mut self.swap_history, snap.swap.used_pct);
        push_history(
            &mut self.gpu_history,
            snap.gpu.as_ref().and_then(|g| g.utilization_pct).unwrap_or(0.0),
        );
        self.push_log(format!(
            "[{}]  {}",
            snap.timestamp.format("%H:%M:%S"),
            snap.summary()
        ));
        self.snapshot = Some(snap);
    }

    fn push_log(&mut self, msg: String) {
        if self.log.len() >= MAX_LOG {
            self.log.pop_front();
        }
        self.log.push_back(msg);
    }

    fn process_count(&self) -> usize {
        self.snapshot.as_ref().map(|s| s.top_processes.len()).unwrap_or(0)
    }

    fn scroll_up(&mut self) {
        self.process_scroll = self.process_scroll.saturating_sub(1);
    }

    fn scroll_down(&mut self) {
        let max = self.process_count().saturating_sub(1);
        if self.process_scroll < max {
            self.process_scroll += 1;
        }
    }
}

fn push_history(buf: &mut VecDeque<f64>, val: f64) {
    if buf.len() >= MAX_HISTORY {
        buf.pop_front();
    }
    buf.push_back(val.clamp(0.0, 100.0));
}

/// Convert a history buffer to chart-ready (x, y) points.
/// Newest point is always at x = MAX_HISTORY − 1 so the curve slides leftward.
fn to_points(buf: &VecDeque<f64>) -> Vec<(f64, f64)> {
    let start_x = (MAX_HISTORY - buf.len()) as f64;
    buf.iter()
        .enumerate()
        .map(|(i, &v)| (start_x + i as f64, v))
        .collect()
}

// ── Entry point ────────────────────────────────────────────────────────────

pub fn run_tui(config: Config) -> Result<()> {
    // Restore terminal on panic
    let orig = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        orig(info);
    }));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    let result = run_loop(&mut terminal, config);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    result
}

// ── Main event loop ────────────────────────────────────────────────────────

fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    config: Config,
) -> Result<()> {
    let mut monitor = SystemMonitor::new();
    let mut app = App::new(config);
    let interval = Duration::from_secs(REFRESH_SECS);

    loop {
        if app.last_refresh.elapsed() >= interval {
            match monitor.snapshot() {
                Ok(snap) => app.ingest_snapshot(snap),
                Err(e) => app.push_log(format!("[ERROR] {}", e)),
            }
            app.last_refresh = Instant::now();
        }

        terminal.draw(|f| render(f, &app))?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(()),
                    KeyCode::Char('c')
                        if key.modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        return Ok(())
                    }
                    KeyCode::Char('r') | KeyCode::Char('R') => {
                        app.last_refresh -= interval;
                    }
                    KeyCode::Up | KeyCode::Char('k') => app.scroll_up(),
                    KeyCode::Down | KeyCode::Char('j') => app.scroll_down(),
                    _ => {}
                }
            }
        }
    }
}

// ── Top-level layout ───────────────────────────────────────────────────────

fn render(f: &mut Frame, app: &App) {
    let area = f.area();

    let core_count = app
        .snapshot
        .as_ref()
        .map(|s| s.cpu.per_core_pct.len())
        .unwrap_or(0);
    let core_rows: u16 = if core_count > 8 { 2 } else { 1 };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),              // title bar
            Constraint::Length(10),             // 2-D curves
            Constraint::Length(core_rows + 2),  // per-core gauges (+ block borders)
            Constraint::Min(7),                 // process table
            Constraint::Length(4),              // activity log
            Constraint::Length(1),              // footer
        ])
        .split(area);

    render_titlebar(f, chunks[0], app);
    render_charts(f, chunks[1], app);
    render_cores(f, chunks[2], app);
    render_processes(f, chunks[3], app);
    render_log(f, chunks[4], app);
    render_footer(f, chunks[5]);
}

// ── Title bar ─────────────────────────────────────────────────────────────

fn render_titlebar(f: &mut Frame, area: Rect, app: &App) {
    let (pressure_str, pressure_color) = app
        .snapshot
        .as_ref()
        .map(|s| match s.memory_pressure {
            MemoryPressureLevel::Normal => ("Normal", Color::Green),
            MemoryPressureLevel::Warning => ("Warning", Color::Yellow),
            MemoryPressureLevel::Critical => ("CRITICAL", Color::Red),
            MemoryPressureLevel::Unknown => ("Unknown", Color::DarkGray),
        })
        .unwrap_or(("Loading…", Color::DarkGray));

    let thermal = app
        .snapshot
        .as_ref()
        .map(|s| match s.thermal_state {
            ThermalState::Nominal => "Nominal",
            ThermalState::Fair => "Fair",
            ThermalState::Serious => "Serious",
            ThermalState::Critical => "CRITICAL",
            ThermalState::Unknown => "Unknown",
        })
        .unwrap_or("—");

    let ts = app
        .snapshot
        .as_ref()
        .map(|s| s.timestamp.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "—".to_string());

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " DeepSpeed ",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  Pressure: "),
            Span::styled(
                pressure_str,
                Style::default().fg(pressure_color).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  Thermal: "),
            Span::raw(thermal),
            Span::raw(format!(
                "  │  AI: {}/{}  │  {}",
                app.config.ai.provider, app.config.ai.model, ts
            )),
        ])),
        area,
    );
}

// ── 2-D curve charts ───────────────────────────────────────────────────────

fn render_charts(f: &mut Frame, area: Rect, app: &App) {
    // Decide third panel: GPU utilisation if available, else Swap
    let has_gpu_util = app
        .snapshot
        .as_ref()
        .and_then(|s| s.gpu.as_ref())
        .and_then(|g| g.utilization_pct)
        .is_some();

    let gpu_name = app
        .snapshot
        .as_ref()
        .and_then(|s| s.gpu.as_ref())
        .map(|g| g.name.as_str())
        .unwrap_or("No GPU");

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
            Constraint::Ratio(1, 3),
        ])
        .split(area);

    // ── CPU chart ─────────────────────────────────────────────────────────
    let cpu_data = to_points(&app.cpu_history);
    let cpu_val = app
        .snapshot
        .as_ref()
        .map(|s| s.cpu.usage_pct)
        .unwrap_or(0.0);
    draw_chart(
        f,
        cols[0],
        &format!(" CPU  {:.1}% ", cpu_val),
        &[(&cpu_data, "CPU", gauge_color(cpu_val))],
    );

    // ── Memory chart (RAM + Swap overlay) ─────────────────────────────────
    let mem_data = to_points(&app.mem_history);
    let swap_data = to_points(&app.swap_history);
    let mem_val = app
        .snapshot
        .as_ref()
        .map(|s| s.memory.used_pct)
        .unwrap_or(0.0);
    let swap_val = app
        .snapshot
        .as_ref()
        .map(|s| s.swap.used_pct)
        .unwrap_or(0.0);
    draw_chart(
        f,
        cols[1],
        &format!(" RAM  {:.1}%  Swap  {:.1}% ", mem_val, swap_val),
        &[
            (&mem_data, "RAM", gauge_color(mem_val)),
            (&swap_data, "Swap", Color::Yellow),
        ],
    );

    // ── GPU or Swap chart ─────────────────────────────────────────────────
    if has_gpu_util {
        let gpu_data = to_points(&app.gpu_history);
        let gpu_val = app
            .snapshot
            .as_ref()
            .and_then(|s| s.gpu.as_ref())
            .and_then(|g| g.utilization_pct)
            .unwrap_or(0.0);
        draw_chart(
            f,
            cols[2],
            &format!(" GPU  {:.1}% ", gpu_val),
            &[(&gpu_data, "GPU", Color::Magenta)],
        );
    } else {
        // Show GPU name / VRAM info as a placeholder
        let vram = app
            .snapshot
            .as_ref()
            .and_then(|s| s.gpu.as_ref())
            .and_then(|g| g.vram_total_mb)
            .map(|v| format!("  {} MB VRAM", v))
            .unwrap_or_default();
        let title = format!(" GPU — {}{}  (root req.) ", gpu_name, vram);
        // Draw swap again so the panel is not empty
        draw_chart(
            f,
            cols[2],
            &title,
            &[(&swap_data, "Swap", Color::Yellow)],
        );
    }
}

/// Draw a single history chart with one or two overlaid curves.
fn draw_chart(
    f: &mut Frame,
    area: Rect,
    title: &str,
    series: &[(&Vec<(f64, f64)>, &str, Color)],
) {
    let max_x = (MAX_HISTORY - 1) as f64;
    let total_secs = MAX_HISTORY as u64 * REFRESH_SECS;
    let half_secs = total_secs / 2;

    let datasets: Vec<Dataset> = series
        .iter()
        .map(|(data, name, color)| {
            Dataset::default()
                .name(*name)
                .marker(symbols::Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(*color))
                .data(data)
        })
        .collect();

    f.render_widget(
        Chart::new(datasets)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(Span::styled(title, Style::default().add_modifier(Modifier::BOLD))),
            )
            .x_axis(
                Axis::default()
                    .bounds([0.0, max_x])
                    .style(Style::default().fg(Color::DarkGray))
                    .labels(vec![
                        Span::raw(format!("-{}s", total_secs)),
                        Span::raw(format!("-{}s", half_secs)),
                        Span::raw("now"),
                    ]),
            )
            .y_axis(
                Axis::default()
                    .bounds([0.0, 100.0])
                    .style(Style::default().fg(Color::DarkGray))
                    .labels(vec![
                        Span::raw("  0%"),
                        Span::raw(" 50%"),
                        Span::raw("100%"),
                    ]),
            ),
        area,
    );
}

// ── Per-core CPU gauges ────────────────────────────────────────────────────

fn render_cores(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" CPU Cores ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let snap = match &app.snapshot {
        Some(s) if !s.cpu.per_core_pct.is_empty() => s,
        _ => return,
    };

    let cores = &snap.cpu.per_core_pct;
    let per_row: usize = 8.min(cores.len());
    let num_rows = if cores.len() > per_row { 2 } else { 1 };

    let row_constraints: Vec<Constraint> =
        (0..num_rows).map(|_| Constraint::Length(1)).collect();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(row_constraints)
        .split(inner);

    for (row_idx, row_area) in rows.iter().enumerate() {
        let start = row_idx * per_row;
        let end = (start + per_row).min(cores.len());
        let slice = &cores[start..end];
        if slice.is_empty() {
            break;
        }
        let n = slice.len() as u32;
        let col_constraints: Vec<Constraint> =
            (0..n).map(|_| Constraint::Ratio(1, n)).collect();
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(col_constraints)
            .split(*row_area);

        for (i, (&pct, &col)) in slice.iter().zip(cols.iter()).enumerate() {
            let pct_u16 = pct.clamp(0.0, 100.0) as u16;
            f.render_widget(
                Gauge::default()
                    .gauge_style(Style::default().fg(gauge_color(pct)))
                    .label(format!("C{} {:2}%", start + i, pct_u16))
                    .percent(pct_u16),
                col,
            );
        }
    }
}

// ── Process table ──────────────────────────────────────────────────────────

fn render_processes(f: &mut Frame, area: Rect, app: &App) {
    let snap = match &app.snapshot {
        Some(s) => s,
        None => {
            f.render_widget(
                Block::default().borders(Borders::ALL).title(" Processes "),
                area,
            );
            return;
        }
    };

    let header = Row::new([
        Cell::from("#"),
        Cell::from("Process"),
        Cell::from("Memory"),
        Cell::from("CPU%"),
        Cell::from("PID"),
        Cell::from("Age"),
        Cell::from("Nice"),
    ])
    .style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )
    .height(1);

    let rows: Vec<Row> = snap
        .top_processes
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mem_style = if p.memory_mb >= 1000 {
                Style::default().fg(Color::Red)
            } else if p.memory_mb >= 400 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };
            Row::new([
                Cell::from(format!("{}", i + 1)),
                Cell::from(p.name.clone()),
                Cell::from(format!("{} MB", p.memory_mb)).style(mem_style),
                Cell::from(format!("{:.1}", p.cpu_pct)),
                Cell::from(format!("{}", p.pid)),
                Cell::from(format_age(p.run_time_secs)),
                Cell::from(format!("{}", p.nice)),
            ])
        })
        .collect();

    let title = format!(" Processes ({}) — ↑↓ / j k ", snap.top_processes.len());
    let mut state = TableState::default();
    state.select(Some(app.process_scroll));

    f.render_stateful_widget(
        Table::new(
            rows,
            [
                Constraint::Length(3),
                Constraint::Min(22),
                Constraint::Length(10),
                Constraint::Length(6),
                Constraint::Length(7),
                Constraint::Length(8),
                Constraint::Length(5),
            ],
        )
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(title))
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED)),
        area,
        &mut state,
    );
}

// ── Activity log ───────────────────────────────────────────────────────────

fn render_log(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Activity Log ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let height = inner.height as usize;
    let lines: Vec<Line> = app
        .log
        .iter()
        .rev()
        .take(height)
        .rev()
        .map(|s| Line::from(s.as_str()))
        .collect();

    f.render_widget(Paragraph::new(lines), inner);
}

// ── Footer ─────────────────────────────────────────────────────────────────

fn render_footer(f: &mut Frame, area: Rect) {
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" q", Style::default().fg(Color::Yellow)),
            Span::raw(":quit  "),
            Span::styled("r", Style::default().fg(Color::Yellow)),
            Span::raw(":refresh  "),
            Span::styled("↑↓ / j k", Style::default().fg(Color::Yellow)),
            Span::raw(":scroll processes  "),
            Span::styled("Ctrl+C", Style::default().fg(Color::Yellow)),
            Span::raw(":exit"),
        ])),
        area,
    );
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn gauge_color(pct: f64) -> Color {
    if pct >= 90.0 {
        Color::Red
    } else if pct >= 75.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn format_age(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    }
}
