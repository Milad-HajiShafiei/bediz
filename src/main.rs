use std::env;
use std::fs;
use std::io::{self, Stdout, Write};
use std::path::{Path, PathBuf};

use image::DynamicImage;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};

const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "webp", "tiff", "tif", "ico", "qoi",
];

// ---------------------------------------------------------------------------
// Render method
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenderMethod {
    HalfBlock,
    Ascii,
    Sixel,
}

impl RenderMethod {
    fn all() -> &'static [RenderMethod] {
        &[
            RenderMethod::HalfBlock,
            RenderMethod::Ascii,
            RenderMethod::Sixel,
        ]
    }

    fn label(&self) -> &str {
        match self {
            RenderMethod::HalfBlock => "Half-Block (▀)",
            RenderMethod::Ascii => "ASCII Art",
            RenderMethod::Sixel => "Sixel Graphics",
        }
    }

    fn description(&self) -> &str {
        match self {
            RenderMethod::HalfBlock => "Full color using ▀ characters with ANSI fg/bg",
            RenderMethod::Ascii => "Grayscale characters mapped by brightness",
            RenderMethod::Sixel => {
                "Pixel-perfect via Sixel escape sequences (requires terminal support)"
            }
        }
    }
}

impl std::fmt::Display for RenderMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label())
    }
}

// ---------------------------------------------------------------------------
// Directory entry
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum DirEntry {
    Dir(String),
    File(String),
}

impl DirEntry {
    fn name(&self) -> &str {
        match self {
            DirEntry::Dir(name) | DirEntry::File(name) => name,
        }
    }
}

fn is_image_file(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

struct App {
    dir: PathBuf,
    entries: Vec<DirEntry>,
    list_state: ListState,
    viewer: Option<ImageViewer>,
    modal: Option<Modal>,
    modal_selection: usize,
    render_method: RenderMethod,
    /// Cached rendered output (lines of spans) for HalfBlock/ASCII
    cached_render: Option<Vec<Line<'static>>>,
    /// Cached raw Sixel escape sequence string
    cached_sixel: Option<String>,
    /// Terminal position and size used for the cached Sixel output
    sixel_area: Option<Rect>,
    /// Terminal size when the cache was built: (width, height)
    cached_size: Option<(u16, u16)>,
    /// Whether we need to re-render the image
    needs_render: bool,
}

struct ImageViewer {
    img: DynamicImage,
    file_path: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Modal {
    RenderMethodPicker,
}

impl App {
    fn new_file_picker(dir: PathBuf) -> Self {
        let mut app = App {
            dir,
            entries: Vec::new(),
            list_state: ListState::default(),
            viewer: None,
            modal: None,
            modal_selection: 0,
            render_method: RenderMethod::HalfBlock,
            cached_render: None,
            cached_sixel: None,
            sixel_area: None,
            cached_size: None,
            needs_render: false,
        };
        app.refresh_entries();
        app
    }

    fn refresh_entries(&mut self) {
        self.entries.clear();

        if self.dir.parent().is_some() {
            self.entries.push(DirEntry::Dir("..".to_string()));
        }

        if let Ok(read_dir) = fs::read_dir(&self.dir) {
            let mut dirs: Vec<DirEntry> = Vec::new();
            let mut files: Vec<DirEntry> = Vec::new();

            for entry in read_dir.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                if let Ok(ft) = entry.file_type() {
                    if ft.is_dir() {
                        dirs.push(DirEntry::Dir(name));
                    } else if ft.is_file() && is_image_file(&name) {
                        files.push(DirEntry::File(name));
                    }
                }
            }

            dirs.sort_by(|a, b| a.name().to_lowercase().cmp(&b.name().to_lowercase()));
            files.sort_by(|a, b| a.name().to_lowercase().cmp(&b.name().to_lowercase()));
            self.entries.extend(dirs);
            self.entries.extend(files);
        }

        self.list_state.select(Some(0));
    }

    fn selected_path(&self) -> Option<PathBuf> {
        let idx = self.list_state.selected()?;
        let entry = self.entries.get(idx)?;
        match entry {
            DirEntry::Dir(name) => {
                if name == ".." {
                    self.dir.parent().map(|p| p.to_path_buf())
                } else {
                    Some(self.dir.join(name))
                }
            }
            DirEntry::File(name) => Some(self.dir.join(name)),
        }
    }

    /// Returns true if app should quit
    fn handle_key(&mut self, code: KeyCode) -> bool {
        // Modal takes priority
        if self.modal.is_some() {
            return self.handle_modal_key(code);
        }

        // Image viewer mode
        if self.viewer.is_some() {
            return self.handle_viewer_key(code);
        }

        // File picker mode
        self.handle_picker_key(code)
    }

    fn handle_modal_key(&mut self, code: KeyCode) -> bool {
        match self.modal {
            Some(Modal::RenderMethodPicker) => {
                let methods = RenderMethod::all();
                let total = methods.len();

                match code {
                    KeyCode::Esc | KeyCode::Char('m') => {
                        self.modal = None;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.modal_selection = self.modal_selection.saturating_sub(1);
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.modal_selection = (self.modal_selection + 1).min(total - 1);
                    }
                    KeyCode::Enter => {
                        self.render_method = methods[self.modal_selection];
                        self.cached_render = None;
                        self.cached_sixel = None;
                        self.sixel_area = None;
                        self.needs_render = true;
                        self.modal = None;
                    }
                    _ => {}
                }
                false
            }
            None => false,
        }
    }

    fn handle_viewer_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => true,
            KeyCode::Char('b') => {
                self.viewer = None;
                false
            }
            KeyCode::Char('m') => {
                self.modal = Some(Modal::RenderMethodPicker);
                // Pre-select the current method
                self.modal_selection = RenderMethod::all()
                    .iter()
                    .position(|m| *m == self.render_method)
                    .unwrap_or(0);
                false
            }
            _ => false,
        }
    }

    fn handle_picker_key(&mut self, code: KeyCode) -> bool {
        let total = self.entries.len();
        match code {
            KeyCode::Char('q') | KeyCode::Esc => true,

            KeyCode::Up | KeyCode::Char('k') => {
                let i = self.list_state.selected().unwrap_or(0);
                if i > 0 {
                    self.list_state.select(Some(i - 1));
                }
                false
            }

            KeyCode::Down | KeyCode::Char('j') => {
                let i = self.list_state.selected().unwrap_or(0);
                if total > 0 && i < total - 1 {
                    self.list_state.select(Some(i + 1));
                }
                false
            }

            KeyCode::Home => {
                self.list_state.select(Some(0));
                false
            }

            KeyCode::End => {
                if total > 0 {
                    self.list_state.select(Some(total - 1));
                }
                false
            }

            KeyCode::PageUp => {
                let i = self.list_state.selected().unwrap_or(0);
                self.list_state.select(Some(i.saturating_sub(20)));
                false
            }

            KeyCode::PageDown => {
                let i = self.list_state.selected().unwrap_or(0);
                self.list_state
                    .select(Some((i + 20).min(total.saturating_sub(1))));
                false
            }

            KeyCode::Enter => {
                if let Some(path) = self.selected_path() {
                    if path.is_dir() {
                        self.dir = path;
                        self.refresh_entries();
                    } else if path.is_file() {
                        if let Ok(img) = image::open(&path) {
                            self.viewer = Some(ImageViewer {
                                img,
                                file_path: path.to_string_lossy().to_string(),
                            });
                            self.cached_render = None;
                            self.cached_sixel = None;
                            self.sixel_area = None;
                            self.needs_render = true;
                        }
                    }
                }
                false
            }

            _ => false,
        }
    }

    // ---- Drawing ----

    fn draw(&self, f: &mut ratatui::Frame) {
        match &self.viewer {
            Some(viewer) => {
                self.draw_image_viewer(f, viewer);
                if self.modal.is_some() {
                    self.draw_modal(f);
                }
            }
            None => {
                self.draw_file_picker(f);
            }
        }
    }

    fn draw_file_picker(&self, f: &mut ratatui::Frame) {
        let area = f.area();
        let chunks = Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(area);

        let list_area = chunks[0];
        let status_area = chunks[1];

        let items: Vec<ListItem> = self
            .entries
            .iter()
            .map(|entry| {
                let line = match entry {
                    DirEntry::Dir(name) => {
                        let icon = if name == ".." { "📁 .." } else { "📁" };
                        Line::from(Span::styled(
                            format!("  {icon} {name}/"),
                            Style::default().fg(Color::Yellow),
                        ))
                    }
                    DirEntry::File(name) => Line::from(Span::styled(
                        format!("  🖼  {name}"),
                        Style::default().fg(Color::White),
                    )),
                };
                ListItem::new(line)
            })
            .collect();

        let title = format!(" 📂 {} ", self.dir.display());
        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(title))
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▸ ");

        let mut state = self.list_state.clone();
        f.render_stateful_widget(list, list_area, &mut state);

        let total_dirs = self
            .entries
            .iter()
            .filter(|e| matches!(e, DirEntry::Dir(_)))
            .count();
        let total_files = self.entries.len().saturating_sub(total_dirs);
        let status = Paragraph::new(Line::from(vec![
            Span::styled(
                " ↑↓:Navigate ",
                Style::default().bg(Color::DarkGray).fg(Color::White),
            ),
            Span::styled(
                " Enter:Open ",
                Style::default().bg(Color::DarkGray).fg(Color::Green),
            ),
            Span::styled(
                " q:Quit ",
                Style::default().bg(Color::DarkGray).fg(Color::White),
            ),
            Span::styled(
                format!(" 📁{total_dirs} 🖼{total_files} "),
                Style::default().bg(Color::DarkGray).fg(Color::Cyan),
            ),
        ]));
        f.render_widget(status, status_area);
    }

    fn draw_image_viewer(&self, f: &mut ratatui::Frame, viewer: &ImageViewer) {
        let area = f.area();
        let chunks = Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(area);

        let image_area = chunks[0];
        let status_area = chunks[1];

        // Render using cached output or show loading
        match self.render_method {
            RenderMethod::Sixel => {
                // Sixel data is written directly to terminal after ratatui flush.
                // Show a blank area here — the Sixel bitmap will overwrite it.
                if self.cached_sixel.is_none() {
                    let loading = Paragraph::new(Line::from(vec![Span::styled(
                        " Converting image... ",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    )]))
                    .alignment(Alignment::Center);
                    f.render_widget(loading, image_area);
                }
                // When cached_sixel is Some, render nothing — Sixel will overwrite.
            }
            _ => match &self.cached_render {
                Some(lines) => {
                    let paragraph = Paragraph::new(lines.clone());
                    f.render_widget(paragraph, image_area);
                }
                None => {
                    let loading = Paragraph::new(Line::from(vec![Span::styled(
                        " Converting image... ",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    )]))
                    .alignment(Alignment::Center);
                    f.render_widget(loading, image_area);
                }
            },
        }

        let method_label = self.render_method.label();
        let status = Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" m:Method({method_label}) "),
                Style::default().bg(Color::DarkGray).fg(Color::Magenta),
            ),
            Span::styled(
                " b:Back ",
                Style::default().bg(Color::DarkGray).fg(Color::White),
            ),
            Span::styled(
                " q:Quit ",
                Style::default().bg(Color::DarkGray).fg(Color::White),
            ),
            Span::styled(
                format!(" {}x{} ", viewer.img.width(), viewer.img.height()),
                Style::default().bg(Color::DarkGray).fg(Color::Yellow),
            ),
            Span::styled(
                format!(" {} ", viewer.file_path),
                Style::default().bg(Color::DarkGray).fg(Color::Cyan),
            ),
        ]));
        f.render_widget(status, status_area);
    }

    fn draw_modal(&self, f: &mut ratatui::Frame) {
        let area = f.area();

        // Center the modal
        let popup_width = 82.min(area.width.saturating_sub(4));
        let popup_height = 10.min(area.height.saturating_sub(4));
        let x = (area.width.saturating_sub(popup_width)) / 2;
        let y = (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(x, y, popup_width, popup_height);

        // Clear the area behind the modal
        f.render_widget(Clear, popup_area);

        match self.modal {
            Some(Modal::RenderMethodPicker) => {
                self.draw_render_method_picker(f, popup_area);
            }
            None => {}
        }
    }

    fn draw_render_method_picker(&self, f: &mut ratatui::Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Title + separator
                Constraint::Min(1),    // Options
                Constraint::Length(2), // Footer
            ])
            .split(area);

        // Title
        let title = Paragraph::new(Line::from(vec![Span::styled(
            "  🎨 Render Method  ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )]))
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
                .border_style(Style::default().fg(Color::Cyan)),
        );
        f.render_widget(title, chunks[0]);

        // Options list
        let methods = RenderMethod::all();
        let items: Vec<ListItem> = methods
            .iter()
            .map(|m| {
                let is_selected = *m == self.render_method;
                let (icon, style) = if is_selected {
                    (
                        "●",
                        Style::default()
                            .fg(Color::Green)
                            .add_modifier(Modifier::BOLD),
                    )
                } else {
                    ("○", Style::default().fg(Color::DarkGray))
                };

                let name_style = if is_selected {
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                let desc_style = Style::default().fg(Color::DarkGray);

                let line = Line::from(vec![
                    Span::styled(format!("  {icon} "), style),
                    Span::styled(m.label().to_string(), name_style),
                    Span::raw("  "),
                    Span::styled(m.description(), desc_style),
                ]);
                ListItem::new(line)
            })
            .collect();

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::LEFT | Borders::RIGHT)
                    .border_style(Style::default().fg(Color::Cyan)),
            )
            .highlight_style(
                Style::default()
                    .bg(Color::DarkGray)
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▸ ");

        let current_idx = self.modal_selection;
        let mut state = ListState::default();
        state.select(Some(current_idx));
        f.render_stateful_widget(list, chunks[1], &mut state);

        // Footer
        let footer = Paragraph::new(Line::from(vec![
            Span::styled(" ↑↓:Select ", Style::default().fg(Color::White)),
            Span::styled(" Enter:Confirm ", Style::default().fg(Color::Green)),
            Span::styled(" Esc:Cancel ", Style::default().fg(Color::Red)),
        ]))
        .alignment(Alignment::Center)
        .block(
            Block::default()
                .borders(Borders::BOTTOM | Borders::LEFT | Borders::RIGHT)
                .border_style(Style::default().fg(Color::Cyan)),
        );
        f.render_widget(footer, chunks[2]);
    }
}

// ---------------------------------------------------------------------------
// Render methods
// ---------------------------------------------------------------------------

/// Half-block: each cell = 2 vertical pixels via fg/bg color
fn image_to_half_blocks(img: &DynamicImage, width: u32, height: u32) -> Vec<Line<'static>> {
    if width == 0 || height == 0 {
        return vec![];
    }

    let resized = img.resize_exact(width, height * 2, image::imageops::FilterType::Nearest);
    let rgba = resized.to_rgba8();
    let mut lines = Vec::with_capacity(height as usize);

    for y in (0..height * 2).step_by(2) {
        let mut spans = Vec::with_capacity(width as usize);
        for x in 0..width {
            let upper = rgba.get_pixel(x, y);
            let lower = if y + 1 < height * 2 {
                rgba.get_pixel(x, y + 1)
            } else {
                rgba.get_pixel(x, y)
            };
            spans.push(Span::styled(
                "▀",
                Style::default()
                    .fg(Color::Rgb(upper[0], upper[1], upper[2]))
                    .bg(Color::Rgb(lower[0], lower[1], lower[2])),
            ));
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// ASCII art: map brightness to characters
fn image_to_ascii(img: &DynamicImage, width: u32, height: u32) -> Vec<Line<'static>> {
    if width == 0 || height == 0 {
        return vec![];
    }

    // ASCII chars from darkest to brightest
    const ASCII_CHARS: &[u8] = b" .:-=+*#%@";

    let resized = img.resize_exact(width, height, image::imageops::FilterType::Nearest);
    let gray = resized.to_luma8();
    let mut lines = Vec::with_capacity(height as usize);

    for y in 0..height {
        let mut spans = Vec::with_capacity(width as usize);
        for x in 0..width {
            let pixel = gray.get_pixel(x, y)[0];
            let idx = (pixel as usize * (ASCII_CHARS.len() - 1)) / 255;
            let ch = ASCII_CHARS[idx] as char;

            // Give it a slight color tint based on the original image.
            let rgba = resized.to_rgba8();
            let px = rgba.get_pixel(x, y);
            let fg = Color::Rgb(px[0], px[1], px[2]);

            spans.push(Span::styled(ch.to_string(), Style::default().fg(fg)));
        }
        lines.push(Line::from(spans));
    }
    lines
}

/// Encode an RGBA image to a Sixel escape sequence string.
fn encode_sixel(img: &image::RgbaImage) -> String {
    let (width, height) = img.dimensions();
    let width = width as usize;
    let height = height as usize;

    // Quantize to a 6x6x6 color cube (216 colors)
    let palette = build_palette(img);
    let palette_len = palette.len();

    let mut out = String::with_capacity(width * height * 2);

    // Sixel header: ESC P q
    out.push_str("\x1bPq");

    // Define color registers (only the ones we use)
    // We'll define all 216 upfront — the terminal only uses the referenced ones.
    for (i, (r, g, b)) in palette.iter().enumerate() {
        let r_pct = (*r as u32 * 100 / 255) as u8;
        let g_pct = (*g as u32 * 100 / 255) as u8;
        let b_pct = (*b as u32 * 100 / 255) as u8;
        // Sixel color definition: #N;2;R%;G%;B%
        out.push_str(&format!("#{i};2;{r_pct};{g_pct};{b_pct}"));
    }

    // Encode in bands of 6 rows
    let num_bands = (height + 5) / 6;

    for band in 0..num_bands {
        let row_start = band * 6;
        let row_end = ((band + 1) * 6).min(height);

        // Build per-color sixel data for this band
        let mut color_used = vec![false; palette_len];
        let mut color_data: Vec<Vec<u8>> = vec![vec![0u8; width]; palette_len];

        for y in row_start..row_end {
            for x in 0..width {
                let px = img.get_pixel(x as u32, y as u32);
                let color_idx = nearest_color(px[0], px[1], px[2], &palette);
                let bit = y - row_start;
                color_data[color_idx][x] |= 1 << bit;
                color_used[color_idx] = true;
            }
        }

        // Output each active color's sixels for this band.
        // For the first color: #Ndata
        // For subsequent colors: $ (CR to col 0) then #Ndata
        let mut first_color = true;
        for c in 0..palette_len {
            if !color_used[c] {
                continue;
            }

            if first_color {
                first_color = false;
            } else {
                // Carriage return to beginning of this sixel row
                out.push('$');
            }

            // Select color register N
            out.push_str(&format!("#{c}"));

            // Write sixel data
            encode_sixel_band(&color_data[c], &mut out);
        }

        // Advance to next sixel row (unless last band)
        if band < num_bands - 1 {
            out.push('-');
        }
    }

    // Sixel string terminator: ESC \
    out.push_str("\x1b\\");

    out
}

/// Build a 256-color palette from the image using uniform quantization
fn build_palette(_img: &image::RgbaImage) -> Vec<(u8, u8, u8)> {
    // Use a 6x6x6 color cube (216 colors) — simple and effective
    let mut palette = Vec::with_capacity(216);
    for r in 0..6 {
        for g in 0..6 {
            for b in 0..6 {
                let rr = if r == 0 { 0 } else { 35 + r * 40 };
                let gg = if g == 0 { 0 } else { 35 + g * 40 };
                let bb = if b == 0 { 0 } else { 35 + b * 40 };
                palette.push((rr as u8, gg as u8, bb as u8));
            }
        }
    }
    palette
}

/// Find nearest color in palette
fn nearest_color(r: u8, g: u8, b: u8, palette: &[(u8, u8, u8)]) -> usize {
    let mut best_idx = 0;
    let mut best_dist = u32::MAX;
    for (i, (pr, pg, pb)) in palette.iter().enumerate() {
        let dr = r as i32 - *pr as i32;
        let dg = g as i32 - *pg as i32;
        let db = b as i32 - *pb as i32;
        let dist = (dr * dr + dg * dg + db * db) as u32;
        if dist < best_dist {
            best_dist = dist;
            best_idx = i;
        }
    }
    best_idx
}

/// Encode a single band (6 rows) of indexed color data to sixel format
fn encode_sixel_band(data: &[u8], out: &mut String) {
    let width = data.len();

    // Sixel encoding: each column gets a character representing 6 vertical bits
    // Characters: 63 (space) to 124 (~)
    //   char = 63 + value  (where value is 0..63)
    let mut i = 0;
    while i < width {
        let val = data[i];

        // Run-length encode same values
        let mut run_len = 1;
        while i + run_len < width && data[i + run_len] == val && run_len < 255 {
            run_len += 1;
        }

        let ch = (val + 63) as char;
        if run_len > 2 {
            // RLE: !count char
            out.push_str(&format!("!{run_len}{ch}"));
        } else {
            for _ in 0..run_len {
                out.push(ch);
            }
        }

        i += run_len;
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let app = if args.len() >= 2 {
        let path = PathBuf::from(&args[1]);
        if path.is_dir() {
            App::new_file_picker(path)
        } else {
            match image::open(&path) {
                Ok(img) => {
                    let dir = path
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| PathBuf::from("."));
                    let mut app = App::new_file_picker(dir);
                    app.viewer = Some(ImageViewer {
                        img,
                        file_path: args[1].clone(),
                    });
                    app
                }
                Err(e) => {
                    eprintln!("Error opening image '{}': {e}", args[1]);
                    disable_raw_mode()?;
                    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
                    terminal.show_cursor()?;
                    std::process::exit(1);
                }
            }
        }
    } else {
        let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        App::new_file_picker(cwd)
    };

    let result = run_app(&mut terminal, app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if let Err(err) = result {
        eprintln!("Error: {err}");
    }

    Ok(())
}

/// Write Sixel data directly to stdout if in Sixel mode.
/// Must be called after `terminal.draw()` so the ratatui UI is flushed first.
fn write_sixel_if_active(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
) -> Result<(), Box<dyn std::error::Error>> {
    if app.render_method != RenderMethod::Sixel {
        return Ok(());
    }
    let Some(ref sixel_data) = app.cached_sixel else {
        return Ok(());
    };

    // Position the bitmap at the image area's origin, below the terminal border.
    let origin = app.sixel_area.unwrap_or(Rect::new(0, 0, 0, 0));
    let stdout = terminal.backend_mut();
    ratatui::crossterm::execute!(
        stdout,
        ratatui::crossterm::cursor::MoveTo(origin.x, origin.y)
    )?;
    stdout.write_all(sixel_data.as_bytes())?;
    stdout.flush()?;

    Ok(())
}

/// Resize image and encode to Sixel for the given terminal dimensions.
fn encode_sixel_resized(img: &DynamicImage, width: u32, height: u32) -> String {
    if width == 0 || height == 0 {
        return String::new();
    }
    let resized = img.resize_exact(width, height, image::imageops::FilterType::Nearest);
    let rgba = resized.to_rgba8();
    encode_sixel(&rgba)
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    mut app: App,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        // If image needs re-rendering, show loading first, then convert
        // Check for terminal resize — invalidate cache
        if app.viewer.is_some() {
            let current_size = terminal.size()?;
            if app.cached_size != Some((current_size.width, current_size.height)) {
                app.cached_render = None;
                app.needs_render = true;
            }
        }

        if app.needs_render && app.viewer.is_some() {
            app.needs_render = false;

            // Draw loading indicator
            terminal.draw(|f| app.draw(f))?;
            terminal.flush()?;

            // Perform the expensive conversion
            let viewer = app.viewer.as_ref().unwrap();
            let area = terminal.size()?;
            app.cached_size = Some((area.width, area.height));
            let image_area = Layout::default()
                .direction(ratatui::layout::Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(1)])
                .split(Rect::new(0, 0, area.width, area.height))[0];
            let image_width = image_area.width as u32;
            let image_height = image_area.height as u32;
            app.sixel_area = Some(image_area);

            match app.render_method {
                RenderMethod::HalfBlock => {
                    let lines = image_to_half_blocks(&viewer.img, image_width, image_height);
                    app.cached_render = Some(lines);
                    app.cached_sixel = None;
                }
                RenderMethod::Ascii => {
                    let lines = image_to_ascii(&viewer.img, image_width, image_height);
                    app.cached_render = Some(lines);
                    app.cached_sixel = None;
                }
                RenderMethod::Sixel => {
                    let sixel = encode_sixel_resized(&viewer.img, image_width, image_height);
                    app.cached_render = None;
                    app.cached_sixel = Some(sixel);
                }
            }
            // Force a redraw from cache after conversion
            terminal.draw(|f| app.draw(f))?;
            write_sixel_if_active(&mut app, terminal)?;
        }

        terminal.draw(|f| app.draw(f))?;
        write_sixel_if_active(&mut app, terminal)?;

        if event::poll(std::time::Duration::from_millis(50))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    if app.handle_key(key.code) {
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}
