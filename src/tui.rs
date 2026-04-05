/// Interactive TUI — CPU (per-core gauges + history curve), Memory, GPU, processes
/// with AI-powered process descriptions, network/disk I/O, load average, and detail view.
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
use crate::monitor::{self, MemoryPressureLevel, SystemMonitor, SystemSnapshot, ThermalState};
use crate::process_intel::{self, ProcessCategory};

const MAX_HISTORY: usize = 60; // number of samples kept (60 x 2 s = 2 min)
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
    net_rx_history: VecDeque<f64>,
    net_tx_history: VecDeque<f64>,
    disk_r_history: VecDeque<f64>,
    disk_w_history: VecDeque<f64>,
    /// Whether the detail panel is open
    detail_open: bool,
    /// Peak values for reference
    peak_cpu: f64,
    peak_mem: f64,
    peak_net_rx: f64,
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
            net_rx_history: VecDeque::with_capacity(MAX_HISTORY),
            net_tx_history: VecDeque::with_capacity(MAX_HISTORY),
            disk_r_history: VecDeque::with_capacity(MAX_HISTORY),
            disk_w_history: VecDeque::with_capacity(MAX_HISTORY),
            detail_open: false,
            peak_cpu: 0.0,
            peak_mem: 0.0,
            peak_net_rx: 0.0,
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
        // Normalize network to KB/s for chart (0-100 scale based on peak)
        let rx_kbs = snap.network.rx_bytes_sec / 1024.0;
        let tx_kbs = snap.network.tx_bytes_sec / 1024.0;
        self.peak_net_rx = self.peak_net_rx.max(rx_kbs).max(tx_kbs).max(1.0);
        push_history(&mut self.net_rx_history, (rx_kbs / self.peak_net_rx * 100.0).min(100.0));
        push_history(&mut self.net_tx_history, (tx_kbs / self.peak_net_rx * 100.0).min(100.0));
        // Disk I/O normalized similarly
        let dr_kbs = snap.disk_io.read_bytes_sec / 1024.0;
        let dw_kbs = snap.disk_io.write_bytes_sec / 1024.0;
        let disk_peak = dr_kbs.max(dw_kbs).max(1.0);
        push_history(&mut self.disk_r_history, (dr_kbs / disk_peak * 100.0).min(100.0));
        push_history(&mut self.disk_w_history, (dw_kbs / disk_peak * 100.0).min(100.0));

        // Track peaks
        self.peak_cpu = self.peak_cpu.max(snap.cpu.usage_pct);
        self.peak_mem = self.peak_mem.max(snap.memory.used_pct);

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

    fn selected_process(&self) -> Option<&monitor::ProcessInfo> {
        self.snapshot
            .as_ref()
            .and_then(|s| s.top_processes.get(self.process_scroll))
    }

    fn toggle_detail(&mut self) {
        self.detail_open = !self.detail_open;
    }
}

fn push_history(buf: &mut VecDeque<f64>, val: f64) {
    if buf.len() >= MAX_HISTORY {
        buf.pop_front();
    }
    buf.push_back(val.clamp(0.0, 100.0));
}

/// Convert a history buffer to chart-ready (x, y) points.
fn to_points(buf: &VecDeque<f64>) -> Vec<(f64, f64)> {
    let start_x = (MAX_HISTORY - buf.len()) as f64;
    buf.iter()
        .enumerate()
        .map(|(i, &v)| (start_x + i as f64, v))
        .collect()
}

// ── Entry point ────────────────────────────────────────────────────────────

pub fn run_tui(config: Config) -> Result<()> {
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
                    KeyCode::Enter | KeyCode::Char('d') => app.toggle_detail(),
                    KeyCode::Esc => { app.detail_open = false; }
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

    // If detail panel is open, allocate space for it
    let detail_height: u16 = if app.detail_open { 8 } else { 0 };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                          // title bar
            Constraint::Length(3),                          // system stats bar
            Constraint::Length(10),                         // 2-D curves (4 panels)
            Constraint::Length(core_rows + 2),              // per-core gauges
            Constraint::Min(7),                             // process table
            Constraint::Length(detail_height),              // detail panel (toggle)
            Constraint::Length(4),                          // activity log
            Constraint::Length(1),                          // footer
        ])
        .split(area);

    render_titlebar(f, chunks[0], app);
    render_stats_bar(f, chunks[1], app);
    render_charts(f, chunks[2], app);
    render_cores(f, chunks[3], app);
    render_processes(f, chunks[4], app);
    if app.detail_open {
        render_detail(f, chunks[5], app);
    }
    render_log(f, chunks[6], app);
    render_footer(f, chunks[7]);
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
        .unwrap_or(("Loading...", Color::DarkGray));

    let thermal = app
        .snapshot
        .as_ref()
        .map(|s| match s.thermal_state {
            ThermalState::Nominal => ("Nominal", Color::Green),
            ThermalState::Fair => ("Fair", Color::Yellow),
            ThermalState::Serious => ("Serious", Color::Red),
            ThermalState::Critical => ("CRITICAL", Color::Red),
            ThermalState::Unknown => ("Unknown", Color::DarkGray),
        })
        .unwrap_or(("--", Color::DarkGray));

    let ts = app
        .snapshot
        .as_ref()
        .map(|s| s.timestamp.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "--".to_string());

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " DeepSpeed ",
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " AI-Powered System Optimizer ",
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw("  Pressure: "),
            Span::styled(
                pressure_str,
                Style::default().fg(pressure_color).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  Thermal: "),
            Span::styled(thermal.0, Style::default().fg(thermal.1)),
            Span::raw(format!(
                "  |  AI: {}/{}  |  {}",
                app.config.ai.provider, app.config.ai.model, ts
            )),
        ])),
        area,
    );
}

// ── System stats bar (new) ────────────────────────────────────────────────

fn render_stats_bar(f: &mut Frame, area: Rect, app: &App) {
    let snap = match &app.snapshot {
        Some(s) => s,
        None => {
            f.render_widget(
                Block::default().borders(Borders::ALL).title(" System Stats "),
                area,
            );
            return;
        }
    };

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 5),
            Constraint::Ratio(1, 5),
            Constraint::Ratio(1, 5),
            Constraint::Ratio(1, 5),
            Constraint::Ratio(1, 5),
        ])
        .split(area);

    // CPU gauge
    let cpu_pct = snap.cpu.usage_pct.clamp(0.0, 100.0) as u16;
    f.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title(format!(
                " CPU {:.1}% @{}MHz ",
                snap.cpu.usage_pct, snap.cpu.freq_mhz
            )))
            .gauge_style(Style::default().fg(gauge_color(snap.cpu.usage_pct)))
            .percent(cpu_pct),
        cols[0],
    );

    // Memory gauge
    let mem_pct = snap.memory.used_pct.clamp(0.0, 100.0) as u16;
    f.render_widget(
        Gauge::default()
            .block(Block::default().borders(Borders::ALL).title(format!(
                " RAM {}/{} MB ",
                snap.memory.used_mb, snap.memory.total_mb
            )))
            .gauge_style(Style::default().fg(gauge_color(snap.memory.used_pct)))
            .percent(mem_pct),
        cols[1],
    );

    // Network I/O
    f.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(" Net ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::styled(
                    format!("D{}/s ", monitor::fmt_bytes(snap.network.rx_bytes_sec as u64)),
                    Style::default().fg(Color::Green),
                ),
                Span::styled(
                    format!("U{}/s", monitor::fmt_bytes(snap.network.tx_bytes_sec as u64)),
                    Style::default().fg(Color::Magenta),
                ),
            ]),
        ])
        .block(Block::default().borders(Borders::ALL)),
        cols[2],
    );

    // Disk I/O
    f.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(" Disk ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::styled(
                    format!("R{}/s ", monitor::fmt_bytes(snap.disk_io.read_bytes_sec as u64)),
                    Style::default().fg(Color::Green),
                ),
                Span::styled(
                    format!("W{}/s", monitor::fmt_bytes(snap.disk_io.write_bytes_sec as u64)),
                    Style::default().fg(Color::Yellow),
                ),
            ]),
        ])
        .block(Block::default().borders(Borders::ALL)),
        cols[3],
    );

    // Load / Processes / Threads
    f.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(" Load ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw(format!(
                    "{:.2} {:.2} {:.2}  ",
                    snap.load_avg[0], snap.load_avg[1], snap.load_avg[2]
                )),
            ]),
        ])
        .block(Block::default().borders(Borders::ALL).title(format!(
            " {}P/{}T ",
            snap.total_processes, snap.total_threads
        ))),
        cols[4],
    );
}

// ── 2-D curve charts ───────────────────────────────────────────────────────

fn render_charts(f: &mut Frame, area: Rect, app: &App) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Ratio(1, 4),
            Constraint::Ratio(1, 4),
            Constraint::Ratio(1, 4),
            Constraint::Ratio(1, 4),
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
        &format!(" CPU  {:.1}% (peak {:.1}%) ", cpu_val, app.peak_cpu),
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
        &format!(" RAM {:.1}%  Swap {:.1}% ", mem_val, swap_val),
        &[
            (&mem_data, "RAM", gauge_color(mem_val)),
            (&swap_data, "Swap", Color::Yellow),
        ],
    );

    // ── GPU or Network chart ──────────────────────────────────────────────
    let has_gpu_util = app
        .snapshot
        .as_ref()
        .and_then(|s| s.gpu.as_ref())
        .and_then(|g| g.utilization_pct)
        .is_some();

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
        // Network I/O chart
        let rx_data = to_points(&app.net_rx_history);
        let tx_data = to_points(&app.net_tx_history);
        draw_chart(
            f,
            cols[2],
            &format!(" Network I/O (peak {}/s) ", monitor::fmt_bytes((app.peak_net_rx * 1024.0) as u64)),
            &[
                (&rx_data, "RX", Color::Green),
                (&tx_data, "TX", Color::Magenta),
            ],
        );
    }

    // ── Disk I/O chart ────────────────────────────────────────────────────
    let dr_data = to_points(&app.disk_r_history);
    let dw_data = to_points(&app.disk_w_history);
    draw_chart(
        f,
        cols[3],
        " Disk I/O ",
        &[
            (&dr_data, "Read", Color::Cyan),
            (&dw_data, "Write", Color::Yellow),
        ],
    );
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

// ── Process table (enhanced with AI descriptions) ─────────────────────────

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
        Cell::from("Cat"),
        Cell::from("Process"),
        Cell::from("AI Description"),
        Cell::from("Memory"),
        Cell::from("CPU%"),
        Cell::from("Thr"),
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
            let (desc, cat, _prof) = process_intel::describe_process(&p.name);
            let cat_color = category_color(cat);

            let mem_style = if p.memory_mb >= 1000 {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            } else if p.memory_mb >= 400 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };

            let cpu_style = if p.cpu_pct >= 80.0 {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            } else if p.cpu_pct >= 50.0 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };

            // Truncate description to fit
            let desc_short = if desc.len() > 30 {
                format!("{}...", &desc[..27])
            } else {
                desc
            };

            Row::new([
                Cell::from(format!("{}", i + 1)),
                Cell::from(cat.icon()).style(Style::default().fg(cat_color)),
                Cell::from(p.name.clone()),
                Cell::from(desc_short).style(Style::default().fg(Color::DarkGray)),
                Cell::from(format!("{} MB", p.memory_mb)).style(mem_style),
                Cell::from(format!("{:.1}", p.cpu_pct)).style(cpu_style),
                Cell::from(format!("{}", p.thread_count)),
                Cell::from(format!("{}", p.pid)),
                Cell::from(format_age(p.run_time_secs)),
                Cell::from(format!("{}", p.nice)),
            ])
        })
        .collect();

    let title = format!(
        " Processes ({}/{}) -- ↑↓ scroll  Enter/d: detail ",
        snap.top_processes.len(),
        snap.total_processes,
    );
    let mut state = TableState::default();
    state.select(Some(app.process_scroll));

    f.render_stateful_widget(
        Table::new(
            rows,
            [
                Constraint::Length(3),   // #
                Constraint::Length(4),   // Cat
                Constraint::Min(16),    // Process
                Constraint::Min(22),    // AI Description
                Constraint::Length(10),  // Memory
                Constraint::Length(6),   // CPU%
                Constraint::Length(4),   // Thr
                Constraint::Length(7),   // PID
                Constraint::Length(8),   // Age
                Constraint::Length(5),   // Nice
            ],
        )
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(title))
        .row_highlight_style(
            Style::default()
                .add_modifier(Modifier::REVERSED)
                .fg(Color::Cyan),
        ),
        area,
        &mut state,
    );
}

// ── Process detail panel (AI-enhanced) ────────────────────────────────────

fn render_detail(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(
            " Process Detail (AI) ",
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ))
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let proc = match app.selected_process() {
        Some(p) => p,
        None => {
            f.render_widget(
                Paragraph::new("  No process selected"),
                inner,
            );
            return;
        }
    };

    let (desc, cat, prof) = process_intel::describe_process(&proc.name);
    let exe_display = proc.exe_path.as_deref().unwrap_or("unknown");

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)])
        .split(inner);

    // Left column: identity & AI description
    let left_lines = vec![
        Line::from(vec![
            Span::styled(" Name: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(&proc.name),
            Span::raw(format!("  (PID {})", proc.pid)),
        ]),
        Line::from(vec![
            Span::styled(" What: ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::styled(&desc, Style::default().fg(Color::White)),
        ]),
        Line::from(vec![
            Span::styled(" Type: ", Style::default().fg(Color::Cyan)),
            Span::styled(cat.label(), Style::default().fg(category_color(cat))),
            Span::raw("  Profile: "),
            Span::styled(prof.label(), Style::default().fg(profile_color(prof))),
        ]),
        Line::from(vec![
            Span::styled(" Path: ", Style::default().fg(Color::DarkGray)),
            Span::raw(if exe_display.len() > 60 {
                format!("...{}", &exe_display[exe_display.len()-57..])
            } else {
                exe_display.to_string()
            }),
        ]),
    ];
    f.render_widget(Paragraph::new(left_lines), cols[0]);

    // Right column: resource details
    let right_lines = vec![
        Line::from(vec![
            Span::styled(" Memory: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{} MB phys, {} MB virt", proc.memory_mb, proc.virtual_memory_mb)),
        ]),
        Line::from(vec![
            Span::styled(" CPU:    ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{:.1}%  Threads: {}  Nice: {}", proc.cpu_pct, proc.thread_count, proc.nice)),
        ]),
        Line::from(vec![
            Span::styled(" Disk:   ", Style::default().fg(Color::Yellow)),
            Span::raw(format!(
                "R {} / W {}",
                monitor::fmt_bytes(proc.disk_read_bytes),
                monitor::fmt_bytes(proc.disk_write_bytes),
            )),
        ]),
        Line::from(vec![
            Span::styled(" IO Pri: ", Style::default().fg(Color::Yellow)),
            Span::raw(&proc.io_priority),
            Span::raw(format!("  Uptime: {}", format_age(proc.run_time_secs))),
        ]),
    ];
    f.render_widget(Paragraph::new(right_lines), cols[1]);
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
            Span::styled("Up/Down/j/k", Style::default().fg(Color::Yellow)),
            Span::raw(":scroll  "),
            Span::styled("Enter/d", Style::default().fg(Color::Yellow)),
            Span::raw(":detail  "),
            Span::styled("Esc", Style::default().fg(Color::Yellow)),
            Span::raw(":close detail  "),
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

fn category_color(cat: ProcessCategory) -> Color {
    match cat {
        ProcessCategory::System => Color::Blue,
        ProcessCategory::Browser => Color::Cyan,
        ProcessCategory::DevTool => Color::Green,
        ProcessCategory::Media => Color::Magenta,
        ProcessCategory::Communication => Color::LightBlue,
        ProcessCategory::Editor => Color::LightGreen,
        ProcessCategory::Server => Color::Yellow,
        ProcessCategory::AI => Color::LightRed,
        ProcessCategory::Database => Color::LightYellow,
        ProcessCategory::Container => Color::LightCyan,
        ProcessCategory::Security => Color::Red,
        ProcessCategory::Desktop => Color::Blue,
        ProcessCategory::FileSystem => Color::DarkGray,
        ProcessCategory::Network => Color::Cyan,
        ProcessCategory::Gaming => Color::Magenta,
        ProcessCategory::Productivity => Color::White,
        ProcessCategory::Unknown => Color::DarkGray,
    }
}

fn profile_color(prof: process_intel::ResourceProfile) -> Color {
    match prof {
        process_intel::ResourceProfile::Light => Color::Green,
        process_intel::ResourceProfile::Moderate => Color::Yellow,
        process_intel::ResourceProfile::Heavy => Color::Red,
        process_intel::ResourceProfile::Bursty => Color::Magenta,
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
