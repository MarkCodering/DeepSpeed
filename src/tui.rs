/// Interactive TUI for DeepSpeed — shows CPU (per-core), GPU, memory, and process table.
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
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Gauge, Paragraph, Row, Table, TableState},
    Frame, Terminal,
};

use crate::config::Config;
use crate::monitor::{GpuInfo, MemoryPressureLevel, SystemMonitor, SystemSnapshot, ThermalState};

const MAX_LOG: usize = 200;
const REFRESH_SECS: u64 = 2;

// ── App state ──────────────────────────────────────────────────────────────

struct App {
    snapshot: Option<SystemSnapshot>,
    log: VecDeque<String>,
    process_scroll: usize,
    last_refresh: Instant,
    config: Config,
}

impl App {
    fn new(config: Config) -> Self {
        let refresh = Duration::from_secs(REFRESH_SECS);
        Self {
            snapshot: None,
            log: VecDeque::with_capacity(MAX_LOG),
            process_scroll: 0,
            // Subtract interval so the first draw triggers an immediate refresh
            last_refresh: Instant::now() - refresh - Duration::from_millis(1),
            config,
        }
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

// ── Entry point ────────────────────────────────────────────────────────────

pub fn run_tui(config: Config) -> Result<()> {
    // Restore terminal on panic so the shell isn't left in raw mode
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(info);
    }));

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

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
    let refresh_interval = Duration::from_secs(REFRESH_SECS);

    loop {
        if app.last_refresh.elapsed() >= refresh_interval {
            match monitor.snapshot() {
                Ok(snap) => {
                    app.push_log(format!(
                        "[{}]  {}",
                        snap.timestamp.format("%H:%M:%S"),
                        snap.summary()
                    ));
                    app.snapshot = Some(snap);
                }
                Err(e) => app.push_log(format!("[ERROR] {}", e)),
            }
            app.last_refresh = Instant::now();
        }

        terminal.draw(|f| render(f, &app))?;

        // Non-blocking key poll — short timeout keeps the refresh ticker alive
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
                        // Force immediate refresh on next iteration
                        app.last_refresh -= refresh_interval;
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

    // Decide how many lines the CPU-core row needs
    let core_rows = if app
        .snapshot
        .as_ref()
        .map(|s| s.cpu.per_core_pct.len() > 4)
        .unwrap_or(false)
    {
        2u16
    } else {
        1u16
    };
    let processor_height = 2 + 1 + core_rows + 1; // borders + overall + cores + gpu

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),              // title bar
            Constraint::Length(processor_height), // processors block
            Constraint::Length(4),              // memory block
            Constraint::Min(8),                 // process table
            Constraint::Length(5),              // activity log
            Constraint::Length(1),              // footer
        ])
        .split(area);

    render_titlebar(f, chunks[0], app);
    render_processors(f, chunks[1], app);
    render_memory(f, chunks[2], app);
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

    let thermal_str = app
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

    let updated = app
        .snapshot
        .as_ref()
        .map(|s| s.timestamp.format("%H:%M:%S").to_string())
        .unwrap_or_else(|| "—".to_string());

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " DeepSpeed ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  Pressure: "),
            Span::styled(
                pressure_str,
                Style::default()
                    .fg(pressure_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  Thermal: "),
            Span::raw(thermal_str),
            Span::raw(format!(
                "  │  AI: {}/{}  │  {}",
                app.config.ai.provider, app.config.ai.model, updated
            )),
        ])),
        area,
    );
}

// ── Processors block (CPU + per-core + GPU) ────────────────────────────────

fn render_processors(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Processors ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let snap = match &app.snapshot {
        Some(s) => s,
        None => return,
    };

    let core_count = snap.cpu.per_core_pct.len();
    // Pack up to 8 cores per row; overflow goes to a second row
    let cores_per_row: usize = 8.min(core_count);
    let core_rows_needed = if core_count > cores_per_row { 2 } else { 1 };

    let mut constraints = vec![Constraint::Length(1)]; // CPU overall
    for _ in 0..core_rows_needed {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1)); // GPU

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    // Overall CPU gauge
    let cpu_pct = snap.cpu.usage_pct.clamp(0.0, 100.0) as u16;
    f.render_widget(
        Gauge::default()
            .gauge_style(Style::default().fg(gauge_color(snap.cpu.usage_pct)))
            .label(format!(
                "CPU  {:.1}%  ({} cores)",
                snap.cpu.usage_pct, snap.cpu.core_count
            ))
            .percent(cpu_pct),
        rows[0],
    );

    // Per-core row(s)
    for row_idx in 0..core_rows_needed {
        let start = row_idx * cores_per_row;
        let end = (start + cores_per_row).min(core_count);
        let slice = &snap.cpu.per_core_pct[start..end];
        if !slice.is_empty() {
            render_core_row(f, rows[1 + row_idx], slice, start);
        }
    }

    // GPU row
    render_gpu_gauge(f, rows[1 + core_rows_needed], snap.gpu.as_ref());
}

fn render_core_row(f: &mut Frame, area: Rect, cores: &[f64], offset: usize) {
    let n = cores.len() as u32;
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(vec![Constraint::Ratio(1, n); cores.len()])
        .split(area);

    for (i, (&pct, &col)) in cores.iter().zip(cols.iter()).enumerate() {
        let pct_u16 = pct.clamp(0.0, 100.0) as u16;
        f.render_widget(
            Gauge::default()
                .gauge_style(Style::default().fg(gauge_color(pct)))
                .label(format!("C{} {:2}%", offset + i, pct_u16))
                .percent(pct_u16),
            col,
        );
    }
}

fn render_gpu_gauge(f: &mut Frame, area: Rect, gpu: Option<&GpuInfo>) {
    match gpu {
        Some(g) => {
            let (pct_u16, label) = match g.utilization_pct {
                Some(u) => {
                    let vram = match (g.vram_used_mb, g.vram_total_mb) {
                        (Some(used), Some(total)) => format!("  {}/{} MB VRAM", used, total),
                        (None, Some(total)) => format!("  {} MB VRAM", total),
                        _ => String::new(),
                    };
                    (
                        u.clamp(0.0, 100.0) as u16,
                        format!("GPU  {:.1}%  {}{}", u, g.name, vram),
                    )
                }
                None => {
                    let vram = g
                        .vram_total_mb
                        .map(|v| format!("  {} MB VRAM", v))
                        .unwrap_or_default();
                    (
                        0,
                        format!(
                            "GPU  N/A  {}{}  (run as root for utilisation)",
                            g.name, vram
                        ),
                    )
                }
            };
            let color = g
                .utilization_pct
                .map(gauge_color)
                .unwrap_or(Color::DarkGray);
            f.render_widget(
                Gauge::default()
                    .gauge_style(Style::default().fg(color))
                    .label(label)
                    .percent(pct_u16),
                area,
            );
        }
        None => {
            f.render_widget(
                Paragraph::new(Span::styled(
                    " GPU  —  not detected",
                    Style::default().fg(Color::DarkGray),
                )),
                area,
            );
        }
    }
}

// ── Memory block ───────────────────────────────────────────────────────────

fn render_memory(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title(" Memory ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let snap = match &app.snapshot {
        Some(s) => s,
        None => return,
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(inner);

    // RAM
    let mem_pct = snap.memory.used_pct.clamp(0.0, 100.0) as u16;
    f.render_widget(
        Gauge::default()
            .gauge_style(Style::default().fg(gauge_color(snap.memory.used_pct)))
            .label(format!(
                "RAM  {}/{} MB  ({:.1}%)  free: {} MB  kernel: {} MB  cached: {} MB",
                snap.memory.used_mb,
                snap.memory.total_mb,
                snap.memory.used_pct,
                snap.memory.available_mb,
                snap.memory.kernel_mb,
                snap.memory.cached_mb,
            ))
            .percent(mem_pct),
        rows[0],
    );

    // Swap — scale color more aggressively (any swap use is notable)
    let swap_pct = snap.swap.used_pct.clamp(0.0, 100.0) as u16;
    f.render_widget(
        Gauge::default()
            .gauge_style(Style::default().fg(gauge_color(snap.swap.used_pct * 1.5)))
            .label(format!(
                "Swap {}/{} MB  ({:.1}%)",
                snap.swap.used_mb, snap.swap.total_mb, snap.swap.used_pct,
            ))
            .percent(swap_pct),
        rows[1],
    );
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

    let title = format!(
        " Processes ({}) — ↑↓ / j k scroll ",
        snap.top_processes.len()
    );
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
