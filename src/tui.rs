//! Interactive terminal UI for NeoSense.
//!
//! The TUI never touches hardware or NBFC directly: every read and every write
//! goes through the `neosensed` supervisor over its IPC socket. Mode selection
//! (Auto, Silent, Balanced, Maximum and thermal profiles) applies immediately;
//! custom manual percentage changes ask for an explicit confirmation. Without a
//! reachable supervisor the interface stays read-only.

use crate::control::ControlReadiness;
use crate::custom_profiles::{CustomProfile, write_private_file};
use crate::diagnostics::{CheckGroup, CheckStatus, DiagnosticReport, diagnose};
use crate::ipc::Client;
use crate::model::now_ms;
use crate::nbfc_config;
use crate::policy::FanRole;
use crate::presets;
use crate::profiles::{ProfileAction, ProfileContext, ProfileOrigin, evaluate_all_with};
use crate::regulation::{RegulationMode, evaluate_profile};
use crate::runtime::{RuntimeView, fan_observations, host_info, read_view};
use crate::supervisor::{SupervisorProbe, SupervisorState};
use crate::warnings::{Severity, from_view as warnings_from_view};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, Padding, Paragraph, Row, Table, TableState, Tabs, Wrap,
};
use serde_json::Value;
use std::io::{IsTerminal, stdout};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Shared visual language for every view.
///
/// Colour carries meaning instead of decoration: cyan marks the brand and the
/// current focus, green marks automatic/active states, yellow marks attention
/// and red marks failure. Everything else uses the terminal's default colour so
/// the interface stays readable on light and dark themes.
mod theme {
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::widgets::{Block, BorderType, Borders, Padding};

    pub const ACCENT: Color = Color::Cyan;
    pub const ACTIVE: Color = Color::Green;
    pub const WARN: Color = Color::Yellow;
    pub const ERROR: Color = Color::Red;
    pub const MUTED: Color = Color::DarkGray;

    pub fn heading() -> Style {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    }

    pub fn selected() -> Style {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    }

    pub fn active() -> Style {
        Style::default().fg(ACTIVE).add_modifier(Modifier::BOLD)
    }

    pub fn muted() -> Style {
        Style::default().fg(MUTED)
    }

    /// Standard inset panel used by every view.
    pub fn panel(title: impl Into<String>) -> Block<'static> {
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(muted())
            .title(title_line(title))
            .padding(Padding::horizontal(1))
    }

    /// Panel with an explicit state colour (active, warning, unavailable).
    pub fn panel_colored(title: impl Into<String>, color: Color) -> Block<'static> {
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(color))
            .title(title_line(title).style(Style::default().fg(color).add_modifier(Modifier::BOLD)))
            .padding(Padding::horizontal(1))
    }

    /// Panel that owns keyboard focus or represents the active choice.
    pub fn panel_focused(title: impl Into<String>) -> Block<'static> {
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(ACCENT))
            .title(title_line(title).style(heading()))
            .padding(Padding::horizontal(1))
    }

    fn title_line(title: impl Into<String>) -> ratatui::text::Line<'static> {
        ratatui::text::Line::from(ratatui::text::Span::styled(
            format!(" {} ", title.into()),
            muted(),
        ))
    }
}

/// NeoSense banner: an ANSI Shadow-inspired wordmark built from solid blocks
/// and box-drawing corners. It is the primary identity on large terminals.
const LOGO_BANNER: [&str; 6] = [
    r#"███╗   ██╗ ███████╗  ██████╗  ███████╗ ███████╗ ███╗   ██╗ ███████╗ ███████╗"#,
    r#"████╗  ██║ ██╔════╝ ██╔═══██╗ ██╔════╝ ██╔════╝ ████╗  ██║ ██╔════╝ ██╔════╝"#,
    r#"██╔██╗ ██║ █████╗   ██║   ██║ ███████╗ █████╗   ██╔██╗ ██║ ███████╗ █████╗  "#,
    r#"██║╚██╗██║ ██╔══╝   ██║   ██║ ╚════██║ ██╔══╝   ██║╚██╗██║ ╚════██║ ██╔══╝  "#,
    r#"██║ ╚████║ ███████╗ ╚██████╔╝ ███████║ ███████╗ ██║ ╚████║ ███████║ ███████╗"#,
    r#"╚═╝  ╚═══╝ ╚══════╝  ╚═════╝  ╚══════╝ ╚══════╝ ╚═╝  ╚═══╝ ╚══════╝ ╚══════╝"#,
];

/// Strict printable-ASCII fallback for the same six-row wordmark. Used when the
/// locale is not UTF-8 or `NEOSENSE_ASCII=1` is set.
const LOGO_BANNER_ASCII: [&str; 6] = [
    r#"###+   ##+ #######+  ######+  #######+ #######+ ###+   ##+ #######+ #######+"#,
    r#"####+  ##| ##+====+ ##+===##+ ##+====+ ##+====+ ####+  ##| ##+====+ ##+====+"#,
    r#"##+##+ ##| #####+   ##|   ##| #######+ #####+   ##+##+ ##| #######+ #####+  "#,
    r#"##|+##+##| ##+==+   ##|   ##| +====##| ##+==+   ##|+##+##| +====##| ##+==+  "#,
    r#"##| +####| #######+ +######++ #######| #######+ ##| +####| #######| #######+"#,
    r#"+=+  +===+ +======+  +=====+  +======+ +======+ +=+  +===+ +======+ +======+"#,
];

/// Compact two-row wordmark used on medium terminals.
const LOGO_MEDIUM: [&str; 2] = [
    r#"█▄ █ █▀▀ █▀▀█ █▀▀ █▀▀ █▄ █ █▀▀ █▀▀"#,
    r#"█ ▀█ ██▄ █▄▄█ ▄▄█ ██▄ █ ▀█ ▄▄█ ██▄"#,
];

/// Product descriptor shown next to the wordmark on large terminals.
const TAGLINE: &str = "LINUX THERMAL MANAGEMENT";

/// Current operating mode as presented to the user. Derived from supervisor
/// state so the TUI never invents a mode the supervisor does not report.
#[derive(Debug, Clone, PartialEq, Eq)]
enum OperatingMode {
    Automatic,
    Preset(usize),
    Profile(String),
    Manual,
    Failsafe,
    Degraded,
    Unknown,
}

impl OperatingMode {
    fn tag(&self) -> (&'static str, Color) {
        match self {
            OperatingMode::Automatic => ("AUTO", theme::ACTIVE),
            OperatingMode::Preset(_) | OperatingMode::Manual => ("MANUAL", theme::WARN),
            OperatingMode::Profile(_) => ("PROFILE", theme::ACCENT),
            OperatingMode::Failsafe => ("FAILSAFE", theme::ERROR),
            OperatingMode::Degraded => ("DEGRADED", theme::WARN),
            OperatingMode::Unknown => ("NO DATA", theme::MUTED),
        }
    }

    fn preset_index(&self) -> Option<usize> {
        match self {
            OperatingMode::Preset(index) => Some(*index),
            _ => None,
        }
    }

    fn profile_name(&self) -> Option<&str> {
        match self {
            OperatingMode::Profile(name) => Some(name),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Dashboard,
    Sensors,
    Fans,
    Profiles,
    System,
    Diagnostics,
    Presets,
}

impl Tab {
    const ALL: [Tab; 7] = [
        Tab::Dashboard,
        Tab::Fans,
        Tab::Presets,
        Tab::Profiles,
        Tab::Sensors,
        Tab::System,
        Tab::Diagnostics,
    ];

    fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }

    fn title(self) -> &'static str {
        match self {
            Tab::Dashboard => "Dashboard",
            Tab::Sensors => "Sensors",
            Tab::Fans => "Fan Control",
            Tab::Profiles => "Profiles",
            Tab::System => "System",
            Tab::Diagnostics => "Diagnostics",
            Tab::Presets => "Presets",
        }
    }

    fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    fn previous(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

pub struct TuiApp {
    tab: Tab,
    view: RuntimeView,
    supervisor: SupervisorProbe,
    diagnostics: Option<DiagnosticReport>,
    control: ControlReadiness,
    socket: Option<PathBuf>,
    last_refresh: Instant,
    refresh_interval: Duration,
    message: Option<String>,
    show_help: bool,
    help_scroll: u16,
    ascii_logo: bool,
    selected_fan: usize,
    editing_fan: Option<(u32, String)>,
    editing_both: Option<String>,
    pending_fan_set: Option<(u32, f64)>,
    pending_both_set: Option<f64>,
    profile_index: usize,
    preset_index: usize,
    pending_editor: Option<EditorRequest>,
    scroll: u16,
    should_quit: bool,
}

#[derive(Debug, Clone)]
enum EditorAction {
    Create,
    Update(String),
}

#[derive(Debug, Clone)]
struct EditorRequest {
    path: PathBuf,
    action: EditorAction,
}

impl TuiApp {
    pub fn new(socket: Option<PathBuf>) -> Self {
        let socket_path = socket
            .clone()
            .unwrap_or_else(crate::daemon::socket_path_from_env);
        let view = read_view(&socket_path);
        let supervisor = view.supervisor.clone();
        let control = crate::diagnostics::control_readiness(&view);
        let mut app = Self {
            tab: Tab::Dashboard,
            view,
            supervisor,
            diagnostics: None,
            control,
            socket: Some(socket_path),
            last_refresh: Instant::now(),
            refresh_interval: Duration::from_secs(2),
            message: None,
            show_help: false,
            help_scroll: 0,
            ascii_logo: !unicode_logo_supported(),
            selected_fan: 0,
            editing_fan: None,
            editing_both: None,
            pending_fan_set: None,
            pending_both_set: None,
            profile_index: 0,
            preset_index: 0,
            pending_editor: None,
            scroll: 0,
            should_quit: false,
        };
        app.refresh_diagnostics();
        app
    }

    #[cfg(test)]
    fn from_view(view: RuntimeView) -> Self {
        let control = crate::diagnostics::control_readiness(&view);
        Self {
            tab: Tab::Dashboard,
            view,
            supervisor: crate::supervisor::probe(std::path::Path::new(
                "/nonexistent-neosense-test.sock",
            )),
            diagnostics: None,
            control,
            socket: None,
            last_refresh: Instant::now(),
            refresh_interval: Duration::from_secs(2),
            message: None,
            show_help: false,
            help_scroll: 0,
            ascii_logo: false,
            selected_fan: 0,
            editing_fan: None,
            editing_both: None,
            pending_fan_set: None,
            pending_both_set: None,
            profile_index: 0,
            preset_index: 0,
            pending_editor: None,
            scroll: 0,
            should_quit: false,
        }
    }

    fn fan_inventory(&self) -> Option<crate::fans::FanInventory> {
        crate::fans::build(&self.view).ok()
    }

    fn supervisor_status(&self) -> Option<&Value> {
        self.supervisor.status.as_ref()
    }

    fn profile_doc(&self) -> Option<&Value> {
        self.supervisor_status()
            .and_then(|status| status.get("profile"))
    }

    /// Resolve the supervisor-reported operating mode. Presets are inferred from
    /// the active fixed-target leases because the supervisor stores the targets,
    /// not the preset name.
    fn operating_mode(&self) -> OperatingMode {
        let Some(status) = self.supervisor_status() else {
            return OperatingMode::Unknown;
        };
        let profile = status.get("profile");
        if profile
            .and_then(|profile| profile.get("active"))
            .and_then(Value::as_bool)
            == Some(true)
        {
            let name = profile
                .and_then(|profile| profile.get("selected"))
                .and_then(Value::as_str)
                .unwrap_or("profile");
            return OperatingMode::Profile(name.to_string());
        }
        let safety = status.get("safety");
        let state = safety
            .and_then(|safety| safety.get("state"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        match state {
            "failsafe" => OperatingMode::Failsafe,
            "degraded" => OperatingMode::Degraded,
            "manual" => match safety.and_then(preset_index_from_safety) {
                Some(index) => OperatingMode::Preset(index),
                None => OperatingMode::Manual,
            },
            "safe_auto" => OperatingMode::Automatic,
            _ => OperatingMode::Unknown,
        }
    }

    fn safety_doc(&self) -> Option<&Value> {
        self.supervisor_status()
            .and_then(|status| status.get("safety"))
    }

    fn safety_reasons(&self) -> String {
        self.safety_doc()
            .and_then(|safety| safety.get("reasons"))
            .and_then(Value::as_array)
            .map(|reasons| {
                reasons
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .unwrap_or_default()
    }

    fn lease_for_fan(&self, index: u32) -> Option<(f64, u64)> {
        let safety = self.safety_doc()?;
        let matches = |lease: &Value| {
            lease.get("fan_index").and_then(Value::as_u64) == Some(u64::from(index))
        };
        let lease = safety
            .get("manual")
            .filter(|lease| !lease.is_null() && matches(lease))
            .or_else(|| {
                safety
                    .get("preset")
                    .and_then(Value::as_array)?
                    .iter()
                    .find(|lease| matches(lease))
            })?;
        Some((
            lease.get("percent").and_then(Value::as_f64)?,
            lease.get("expires_at_ms").and_then(Value::as_u64)?,
        ))
    }

    fn lease_label(&self, index: u32) -> Option<String> {
        let (_, expires_at) = self.lease_for_fan(index)?;
        let remaining_ms = expires_at.saturating_sub(now_ms());
        if remaining_ms == 0 {
            return Some("lease expiring".to_string());
        }
        let seconds = remaining_ms / 1000;
        Some(if seconds >= 60 {
            format!("auto in {}m{:02}s", seconds / 60, seconds % 60)
        } else {
            format!("auto in {seconds}s")
        })
    }

    fn backend_targets(&self) -> Vec<(String, String)> {
        self.view
            .status
            .as_ref()
            .map(|status| {
                status
                    .fans
                    .iter()
                    .map(|fan| {
                        (
                            fan.name.clone(),
                            fan.target_speed
                                .map(|value| format!("{value:.1}%"))
                                .unwrap_or_else(|| "n/a".into()),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn selected_inventory_fan(&self) -> Option<crate::fans::FanInfo> {
        let inventory = self.fan_inventory()?;
        inventory.fans.get(self.selected_fan).cloned()
    }

    fn fan_count(&self) -> usize {
        self.fan_inventory()
            .map(|inventory| inventory.fans.len())
            .unwrap_or(0)
    }

    fn both_available(&self) -> bool {
        self.fan_count() >= 2
    }

    fn selection_is_both(&self) -> bool {
        self.both_available() && self.selected_fan >= self.fan_count()
    }

    fn selection_len(&self) -> usize {
        self.fan_count() + usize::from(self.both_available())
    }

    fn select_previous_row(&mut self) {
        self.selected_fan = self.selected_fan.saturating_sub(1);
    }

    fn select_next_row(&mut self) {
        self.selected_fan = (self.selected_fan + 1).min(self.selection_len().saturating_sub(1));
    }

    fn adjust_selection(&mut self, delta: f64) {
        if self.selection_is_both() {
            self.adjust_both(delta);
        } else {
            self.adjust_selected_fan(delta);
        }
    }

    fn adjust_both(&mut self, delta: f64) {
        let Some(inventory) = self.fan_inventory() else {
            self.message = Some("No discovered fan to adjust".to_string());
            return;
        };
        let current = inventory
            .fans
            .iter()
            .filter_map(|fan| fan.target_percent)
            .next()
            .unwrap_or(50.0);
        let floor = f64::from(crate::policy::SafetyConfig::default().min_percent);
        let percent = (current + delta).clamp(floor, 100.0);
        self.pending_both_set = Some(percent);
        self.message = Some(format!("Set both fans to {percent:.1}%? y/n"));
    }

    fn begin_fan_edit(&mut self) {
        if self.selection_is_both() {
            self.begin_both_edit();
            return;
        }
        let Some(fan) = self.selected_inventory_fan() else {
            self.message = Some("No discovered fan to edit".to_string());
            return;
        };
        let floor = f64::from(crate::policy::SafetyConfig::default().min_percent);
        let initial = fan
            .target_percent
            .or(fan.requested_target_percent)
            .unwrap_or(50.0)
            .clamp(floor, 100.0);
        self.editing_fan = Some((fan.index, String::new()));
        self.message = Some(format!(
            "Fan {} current target {initial:.1}%: type {floor:.0}–100 and press Enter (Esc cancels)",
            fan.index
        ));
    }

    fn handle_fan_editor_key(&mut self, code: KeyCode, mut buffer: String) {
        let Some((index, _)) = self.editing_fan.clone() else {
            return;
        };
        match code {
            KeyCode::Esc => {
                self.editing_fan = None;
                self.message = Some("Fan target edit cancelled".to_string());
            }
            KeyCode::Backspace => {
                buffer.pop();
                self.editing_fan = Some((index, buffer));
            }
            KeyCode::Char('0'..='9') | KeyCode::Char('.') => {
                if let KeyCode::Char(character) = code {
                    buffer.push(character);
                }
                self.editing_fan = Some((index, buffer));
            }
            KeyCode::Enter => {
                let Ok(percent) = buffer.parse::<f64>() else {
                    self.message = Some("Invalid percentage".to_string());
                    return;
                };
                let floor = f64::from(crate::policy::SafetyConfig::default().min_percent);
                if !percent.is_finite() || !(floor..=100.0).contains(&percent) {
                    self.message = Some(format!(
                        "Percentage must be between {floor:.0} and 100; 0% is never allowed"
                    ));
                    return;
                }
                self.editing_fan = None;
                self.pending_fan_set = Some((index, percent));
                self.message = Some(self.fan_target_prompt(index, percent));
            }
            _ => {}
        }
    }

    fn begin_both_edit(&mut self) {
        let Some(inventory) = self.fan_inventory() else {
            self.message = Some("No discovered fans to edit".to_string());
            return;
        };
        if inventory.fans.is_empty() {
            self.message = Some("No discovered fans to edit".to_string());
            return;
        }
        self.editing_both = Some(String::new());
        self.message = Some("Set both fans: type 20–100 and press Enter (Esc cancels)".to_string());
    }

    fn handle_both_editor_key(&mut self, code: KeyCode, mut buffer: String) {
        match code {
            KeyCode::Esc => {
                self.editing_both = None;
                self.message = Some("Both-fan target edit cancelled".to_string());
            }
            KeyCode::Backspace => {
                buffer.pop();
                self.editing_both = Some(buffer);
            }
            KeyCode::Char('0'..='9') | KeyCode::Char('.') => {
                if let KeyCode::Char(character) = code {
                    buffer.push(character);
                }
                self.editing_both = Some(buffer);
            }
            KeyCode::Enter => {
                let Ok(percent) = buffer.parse::<f64>() else {
                    self.message = Some("Invalid percentage".to_string());
                    return;
                };
                let floor = f64::from(crate::policy::SafetyConfig::default().min_percent);
                if !percent.is_finite() || !(floor..=100.0).contains(&percent) {
                    self.message = Some(format!(
                        "Percentage must be between {floor:.0} and 100; 0% is never allowed"
                    ));
                    return;
                }
                self.editing_both = None;
                self.pending_both_set = Some(percent);
                self.message = Some(format!("Set CPU and GPU targets to {percent:.1}%? y/n"));
            }
            _ => {}
        }
    }

    fn confirm_both_set(&mut self, percent: f64) {
        let Some(inventory) = self.fan_inventory() else {
            self.message = Some("Fan discovery unavailable".to_string());
            return;
        };
        if inventory.fans.is_empty() {
            self.message = Some("No discovered fans to edit".to_string());
            return;
        }
        let targets: Vec<Value> = inventory
            .fans
            .iter()
            .map(|fan| serde_json::json!({"fan": fan.index, "percent": percent}))
            .collect();
        let Some(socket) = self.socket.as_ref() else {
            self.message = Some("No supervisor socket configured".to_string());
            return;
        };
        match Client::connect(socket) {
            Ok(mut client) => match client.call(
                "fan.set_many",
                serde_json::json!({"targets": targets, "confirm": true, "dry_run": false}),
            ) {
                Ok(value) => {
                    let verified = value
                        .get("verified")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let partial = value
                        .get("partial")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    self.message = Some(if verified {
                        format!("Both fan targets {percent:.1}% confirmed by backend")
                    } else if partial {
                        format!(
                            "Both fan targets {percent:.1}% partially applied; automatic recovery requested"
                        )
                    } else {
                        format!("Both fan targets {percent:.1}% were not verified")
                    });
                    self.refresh();
                }
                Err(error) => {
                    self.message = Some(error.message);
                    self.refresh();
                }
            },
            Err(error) => self.message = Some(error.message),
        }
    }

    fn adjust_selected_fan(&mut self, delta: f64) {
        let Some(fan) = self.selected_inventory_fan() else {
            self.message = Some("No discovered fan to adjust".to_string());
            return;
        };
        let current = fan
            .target_percent
            .or(fan.requested_target_percent)
            .unwrap_or(50.0);
        let floor = f64::from(crate::policy::SafetyConfig::default().min_percent);
        let percent = (current + delta).clamp(floor, 100.0);
        self.pending_fan_set = Some((fan.index, percent));
        self.message = Some(self.fan_target_prompt(fan.index, percent));
    }

    fn fan_target_prompt(&self, index: u32, percent: f64) -> String {
        let profile_active = self
            .supervisor
            .status
            .as_ref()
            .and_then(|status| status.get("profile"))
            .and_then(|profile| profile.get("active"))
            .and_then(Value::as_bool)
            == Some(true);
        if profile_active {
            format!(
                "Request fan {index} target {percent:.1}%? This ends the active profile. Manual mode is not temperature regulation. y/n"
            )
        } else {
            format!(
                "Request fan {index} target {percent:.1}%? Manual mode is not temperature regulation. y/n"
            )
        }
    }

    fn apply_selected_auto(&mut self) {
        if self.selection_is_both() {
            self.apply_auto();
            return;
        }
        match self.selected_inventory_fan() {
            Some(fan) => self.apply_fan_auto(fan.index),
            None => self.message = Some("No discovered fan selected".to_string()),
        }
    }

    fn apply_profile_at(&mut self, index: usize) {
        let Some(evaluation) = self.evaluations().get(index).cloned() else {
            self.message = Some("Profile no longer available".to_string());
            return;
        };
        if evaluation.profile.action == ProfileAction::SafeAuto {
            self.apply_auto();
            return;
        }
        if evaluation.profile.curve.is_none() {
            self.message = Some(format!(
                "{} has no regulation curve and cannot be applied",
                evaluation.profile.name
            ));
            return;
        }
        if !self.supervisor.state.is_connected() {
            self.message = Some("Supervisor unavailable: TUI is read-only".to_string());
            return;
        }
        let Some(socket) = self.socket.as_ref() else {
            self.message = Some("No supervisor socket configured".to_string());
            return;
        };
        self.message = Some(format!(
            "Applying {} ({})…",
            evaluation.profile.name,
            evaluation.compatibility.label()
        ));
        match Client::connect(socket) {
            Ok(mut client) => match client.call(
                "profile.apply",
                serde_json::json!({"name": evaluation.profile.name, "confirm": true}),
            ) {
                Ok(value) => {
                    let applied = value
                        .get("hardware_applied")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let partial = value
                        .get("partial")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let applied_fans = fan_index_list(value.get("applied_fans"));
                    let blocked_fans = fan_index_list(value.get("blocked_fans"));
                    self.message = Some(if applied {
                        format!(
                            "{} active{} · applied {applied_fans} · blocked {blocked_fans}",
                            evaluation.profile.name,
                            if partial { " (partial)" } else { "" }
                        )
                    } else {
                        value
                            .get("message")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| {
                                format!("{} was not applied", evaluation.profile.name)
                            })
                    });
                    self.refresh();
                }
                Err(error) => {
                    self.message = Some(error.message);
                    self.refresh();
                }
            },
            Err(error) => self.message = Some(error.message),
        }
    }

    fn confirm_fan_set(&mut self, index: u32, percent: f64) {
        let Some(socket) = self.socket.as_ref() else {
            self.message = Some("No supervisor socket configured".to_string());
            return;
        };
        match Client::connect(socket) {
            Ok(mut client) => match client.call(
                "fan.set",
                serde_json::json!({"fan": index, "percent": percent, "confirm": true}),
            ) {
                Ok(value) => {
                    let verified = value
                        .get("verified")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    self.message = Some(
                        value
                            .get("message")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| {
                                if verified {
                                    format!("Fan {index} target {percent:.1}% confirmed by backend")
                                } else {
                                    format!("Fan {index} target was not verified")
                                }
                            }),
                    );
                    self.refresh();
                }
                Err(error) => {
                    self.message = Some(error.message);
                    self.refresh();
                }
            },
            Err(error) => self.message = Some(error.message),
        }
    }

    fn apply_fan_auto(&mut self, index: u32) {
        if !self.supervisor.state.is_connected() {
            self.message = Some("Supervisor unavailable: TUI is read-only".to_string());
            return;
        }
        let Some(socket) = self.socket.as_ref() else {
            self.message = Some("No supervisor socket configured".to_string());
            return;
        };
        match Client::connect(socket) {
            Ok(mut client) => match client.call("fan.auto", serde_json::json!({"fan": index})) {
                Ok(value) => {
                    let verified = value
                        .get("verified")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    self.message = Some(
                        value
                            .get("message")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| {
                                if verified {
                                    format!("Fan {index} reports automatic mode")
                                } else {
                                    format!("Fan {index} automatic mode not confirmed")
                                }
                            }),
                    );
                    self.refresh();
                }
                Err(error) => {
                    self.message = Some(error.message);
                    self.refresh();
                }
            },
            Err(error) => self.message = Some(error.message),
        }
    }

    fn clear_profile(&mut self) {
        if !self.supervisor.state.is_connected() {
            self.message = Some("Supervisor unavailable: TUI is read-only".to_string());
            return;
        }
        let Some(socket) = self.socket.as_ref() else {
            self.message = Some("No supervisor socket configured".to_string());
            return;
        };
        match Client::connect(socket) {
            Ok(mut client) => match client.call("profile.clear", serde_json::json!({})) {
                Ok(value) => {
                    self.message = Some(
                        value
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("Profile deactivated; fans returned to automatic mode")
                            .to_string(),
                    );
                    self.refresh();
                }
                Err(error) => {
                    self.message = Some(error.message);
                    self.refresh();
                }
            },
            Err(error) => self.message = Some(error.message),
        }
    }

    fn custom_profiles(&self) -> Vec<CustomProfile> {
        self.supervisor
            .status
            .as_ref()
            .and_then(|status| status.get("custom"))
            .and_then(|custom| custom.get("profiles"))
            .and_then(|profiles| serde_json::from_value(profiles.clone()).ok())
            .unwrap_or_default()
    }

    fn evaluations(&self) -> Vec<crate::profiles::ProfileEvaluation> {
        evaluate_all_with(
            &ProfileContext::from_view(&self.view),
            &self.custom_profiles(),
        )
    }

    fn refresh(&mut self) {
        let socket = self
            .socket
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new("/nonexistent-neosense-test.sock"));
        self.view = read_view(socket);
        self.supervisor = self.view.supervisor.clone();
        self.control = crate::diagnostics::control_readiness(&self.view);
        self.selected_fan = self
            .selected_fan
            .min(self.selection_len().saturating_sub(1));
        let profile_count = self.evaluations().len();
        self.profile_index = self.profile_index.min(profile_count.saturating_sub(1));
        if self.tab == Tab::Diagnostics {
            self.refresh_diagnostics();
        }
    }

    fn refresh_diagnostics(&mut self) {
        if self.tab == Tab::Diagnostics {
            self.diagnostics = Some(diagnose(&self.view));
        }
    }

    fn select_tab(&mut self, tab: Tab) {
        self.tab = tab;
        self.scroll = 0;
        match tab {
            Tab::Presets => {
                if let Some(index) = self.operating_mode().preset_index() {
                    self.preset_index = index;
                }
            }
            Tab::Profiles => {
                if let Some(name) = self.operating_mode().profile_name().map(str::to_string) {
                    let evaluations = self.evaluations();
                    if let Some(index) = evaluations
                        .iter()
                        .position(|evaluation| evaluation.profile.name == name)
                    {
                        self.profile_index = index;
                    }
                }
            }
            _ => {}
        }
        self.refresh_diagnostics();
    }

    fn maybe_refresh(&mut self) {
        if self.last_refresh.elapsed() >= self.refresh_interval && !self.show_help {
            self.refresh();
            self.last_refresh = Instant::now();
        }
    }

    fn handle_key(&mut self, code: KeyCode) {
        if self.show_help {
            match code {
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => {
                    self.show_help = false;
                    self.help_scroll = 0;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.help_scroll = self.help_scroll.saturating_sub(1);
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.help_scroll = self.help_scroll.saturating_add(1);
                }
                KeyCode::PageUp => {
                    self.help_scroll = self.help_scroll.saturating_sub(5);
                }
                KeyCode::PageDown => {
                    self.help_scroll = self.help_scroll.saturating_add(5);
                }
                KeyCode::Home => self.help_scroll = 0,
                _ => {}
            }
            return;
        }
        if let Some(buffer) = self.editing_both.clone() {
            self.handle_both_editor_key(code, buffer);
            return;
        }
        if let Some((_, buffer)) = self.editing_fan.clone() {
            self.handle_fan_editor_key(code, buffer);
            return;
        }
        if let Some(percent) = self.pending_both_set {
            match code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.pending_both_set = None;
                    self.confirm_both_set(percent);
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.pending_both_set = None;
                    self.message = Some("Both-fan target request cancelled".to_string());
                }
                _ => {}
            }
            return;
        }
        if let Some((index, percent)) = self.pending_fan_set {
            match code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.pending_fan_set = None;
                    self.confirm_fan_set(index, percent);
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.pending_fan_set = None;
                    self.message = Some("Fan target request cancelled".to_string());
                }
                _ => {}
            }
            return;
        }

        self.message = None;
        match code {
            KeyCode::Esc | KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => {
                self.show_help = true;
                self.help_scroll = 0;
            }
            KeyCode::Tab | KeyCode::Right => {
                let tab = self.tab.next();
                self.select_tab(tab);
            }
            KeyCode::BackTab | KeyCode::Left => {
                let tab = self.tab.previous();
                self.select_tab(tab);
            }
            KeyCode::Char('r') => {
                self.refresh();
                self.last_refresh = Instant::now();
                self.message = Some("Refreshed".to_string());
            }
            KeyCode::Char('1') => self.select_tab(Tab::Dashboard),
            KeyCode::Char('2') => self.select_tab(Tab::Fans),
            KeyCode::Char('3') => self.select_tab(Tab::Presets),
            KeyCode::Char('4') => self.select_tab(Tab::Profiles),
            KeyCode::Char('5') => self.select_tab(Tab::Sensors),
            KeyCode::Char('6') => self.select_tab(Tab::System),
            KeyCode::Char('7') => self.select_tab(Tab::Diagnostics),
            KeyCode::Up if self.tab == Tab::Profiles => {
                self.profile_index = self.profile_index.saturating_sub(1);
            }
            KeyCode::Down if self.tab == Tab::Profiles => {
                let count = self.evaluations().len();
                self.profile_index = (self.profile_index + 1).min(count.saturating_sub(1));
            }
            KeyCode::Up if self.tab == Tab::Presets => {
                self.preset_index = self.preset_index.saturating_sub(1);
            }
            KeyCode::Down if self.tab == Tab::Presets => {
                let count = presets::builtins().len();
                self.preset_index = (self.preset_index + 1).min(count.saturating_sub(1));
            }
            KeyCode::Enter if self.tab == Tab::Presets => {
                self.apply_preset(self.preset_index);
            }
            KeyCode::Enter if self.tab == Tab::Profiles => {
                self.apply_profile_at(self.profile_index);
            }
            KeyCode::Up if self.tab == Tab::Fans => self.select_previous_row(),
            KeyCode::Down if self.tab == Tab::Fans => self.select_next_row(),
            KeyCode::Char('m') if self.tab == Tab::Fans => self.begin_fan_edit(),
            KeyCode::Char('b') if self.tab == Tab::Fans => self.begin_both_edit(),
            KeyCode::Char('+') | KeyCode::Char('=') if self.tab == Tab::Fans => {
                self.adjust_selection(5.0)
            }
            KeyCode::Char('-') if self.tab == Tab::Fans => self.adjust_selection(-5.0),
            KeyCode::Char('a') if self.tab == Tab::Fans => self.apply_selected_auto(),
            KeyCode::Char('a') if self.tab == Tab::Profiles => {
                self.apply_profile_at(self.profile_index)
            }
            KeyCode::Char('p') if self.tab == Tab::Profiles => {
                self.preview_profile_at(self.profile_index)
            }
            KeyCode::Char('d') if self.tab == Tab::Profiles => self.clear_profile(),
            KeyCode::Char('e') if self.tab == Tab::Profiles => self.begin_edit_selected(),
            KeyCode::Char('n') if self.tab == Tab::Profiles => self.begin_new_custom(),
            KeyCode::Char('s') if self.tab == Tab::Dashboard => self.apply_preset(0),
            KeyCode::Char('b') if self.tab == Tab::Dashboard => self.apply_preset(1),
            KeyCode::Char('m') if self.tab == Tab::Dashboard => self.apply_preset(2),
            KeyCode::Char('a') => self.apply_auto(),
            KeyCode::Char('A') => self.apply_auto(),
            KeyCode::PageUp
                if matches!(self.tab, Tab::System | Tab::Diagnostics | Tab::Sensors) =>
            {
                self.scroll = self.scroll.saturating_sub(5);
            }
            KeyCode::PageDown
                if matches!(self.tab, Tab::System | Tab::Diagnostics | Tab::Sensors) =>
            {
                self.scroll = self.scroll.saturating_add(5);
            }
            KeyCode::Home if matches!(self.tab, Tab::System | Tab::Diagnostics | Tab::Sensors) => {
                self.scroll = 0;
            }
            KeyCode::Char('k')
                if matches!(self.tab, Tab::System | Tab::Diagnostics | Tab::Sensors) =>
            {
                self.scroll = self.scroll.saturating_sub(1);
            }
            KeyCode::Char('j')
                if matches!(self.tab, Tab::System | Tab::Diagnostics | Tab::Sensors) =>
            {
                self.scroll = self.scroll.saturating_add(1);
            }
            _ => {}
        }
    }

    fn editor_profile(&self, name: &str) -> Option<CustomProfile> {
        self.supervisor
            .status
            .as_ref()
            .and_then(|status| status.get("custom"))
            .and_then(|custom| custom.get("profiles"))
            .and_then(|profiles| {
                serde_json::from_value::<Vec<CustomProfile>>(profiles.clone()).ok()
            })
            .and_then(|profiles| profiles.into_iter().find(|profile| profile.name == name))
    }

    fn editor_path(&self, name: &str) -> PathBuf {
        crate::custom_profiles::editor_path(name)
    }

    fn begin_edit_selected(&mut self) {
        self.message = None;
        let evaluations = self.evaluations();
        let Some(evaluation) = evaluations.get(self.profile_index) else {
            self.message = Some("Profile no longer available".to_string());
            return;
        };
        if evaluation.profile.origin != ProfileOrigin::Custom {
            self.message =
                Some("Built-in curves are read-only; press n to create a custom curve".to_string());
            return;
        }
        if self.socket.is_none() || !self.supervisor.state.is_connected() {
            self.message = Some("Custom profiles are edited through the supervisor".to_string());
            return;
        }
        let name = evaluation.profile.name.clone();
        let Some(profile) = self.editor_profile(&name) else {
            self.message = Some("Custom definition is unavailable; press r to refresh".to_string());
            return;
        };
        let path = self.editor_path(&name);
        match serde_json::to_string_pretty(&profile)
            .map_err(|e| e.to_string())
            .and_then(|json| write_private_file(&path, &json).map_err(|e| e.to_string()))
        {
            Ok(()) => {
                self.message = Some(format!(
                    "Editing {name}; save and exit the editor to validate"
                ));
                self.pending_editor = Some(EditorRequest {
                    path,
                    action: EditorAction::Update(name),
                });
            }
            Err(error) => self.message = Some(format!("Cannot create editor file: {error}")),
        }
    }

    fn begin_new_custom(&mut self) {
        self.message = None;
        if self.socket.is_none() || !self.supervisor.state.is_connected() {
            self.message = Some("Custom profiles are created through the supervisor".to_string());
            return;
        }
        let existing: Vec<String> = self
            .custom_profiles()
            .into_iter()
            .map(|profile| profile.name)
            .collect();
        let mut name = "my-curve".to_string();
        let mut suffix = 2;
        while existing.contains(&name) {
            name = format!("my-curve-{suffix}");
            suffix += 1;
        }
        let profile = CustomProfile {
            name: name.clone(),
            title: "My curve".to_string(),
            description: "User-defined dynamic thermal curve.".to_string(),
            points: vec![
                crate::regulation::CurvePoint {
                    temperature_c: 40.0,
                    percent: 25.0,
                },
                crate::regulation::CurvePoint {
                    temperature_c: 60.0,
                    percent: 45.0,
                },
                crate::regulation::CurvePoint {
                    temperature_c: 80.0,
                    percent: 75.0,
                },
                crate::regulation::CurvePoint {
                    temperature_c: 90.0,
                    percent: 100.0,
                },
            ],
            min_percent: 20.0,
            max_percent: 100.0,
            hysteresis_c: 3.0,
            max_step_percent: 10.0,
            stale_after_ms: 3_000,
            fan_roles: vec![FanRole::Cpu, FanRole::Gpu],
        };
        let path = self.editor_path(&name);
        match serde_json::to_string_pretty(&profile)
            .map_err(|e| e.to_string())
            .and_then(|json| write_private_file(&path, &json).map_err(|e| e.to_string()))
        {
            Ok(()) => {
                self.message = Some(
                    "Edit the new curve, then save and exit to validate and create".to_string(),
                );
                self.pending_editor = Some(EditorRequest {
                    path,
                    action: EditorAction::Create,
                });
            }
            Err(error) => self.message = Some(format!("Cannot create editor file: {error}")),
        }
    }

    fn finish_editor(&mut self, request: EditorRequest) {
        let text = match std::fs::read_to_string(&request.path) {
            Ok(text) => text,
            Err(error) => {
                self.message = Some(format!("Cannot read edited profile: {error}"));
                return;
            }
        };
        let _ = std::fs::remove_file(&request.path);
        let profile: CustomProfile = match serde_json::from_str(&text) {
            Ok(profile) => profile,
            Err(error) => {
                self.message = Some(format!("Edited profile is not valid JSON: {error}"));
                return;
            }
        };
        if let Err(error) = profile.validate() {
            self.message = Some(format!("Edited profile is invalid: {error}"));
            return;
        }
        if let EditorAction::Update(old_name) = &request.action {
            if profile.name != *old_name {
                self.message = Some(
                    "Renaming in the editor is not supported; use `profile custom rename`"
                        .to_string(),
                );
                return;
            }
        }
        let Some(socket) = self.socket.as_ref() else {
            self.message = Some("No supervisor socket configured".to_string());
            return;
        };
        let method = match &request.action {
            EditorAction::Create => "custom.create",
            EditorAction::Update(_) => "custom.update",
        };
        match Client::connect(socket) {
            Ok(mut client) => match client.call(method, serde_json::json!({"profile": profile})) {
                Ok(_) => {
                    self.message = Some(format!(
                        "Custom curve {} through the supervisor",
                        if matches!(request.action, EditorAction::Create) {
                            "created"
                        } else {
                            "updated"
                        }
                    ));
                    self.refresh();
                }
                Err(error) => self.message = Some(error.message),
            },
            Err(error) => self.message = Some(error.message),
        }
    }

    fn preview_profile_at(&mut self, index: usize) {
        let evaluations = self.evaluations();
        let Some(evaluation) = evaluations.get(index) else {
            self.message = Some("Profile no longer available".to_string());
            return;
        };
        if evaluation.profile.action == ProfileAction::SafeAuto {
            self.message =
                Some("Automatic mode has no curve; press Enter to restore it".to_string());
            return;
        }
        let Some(socket) = self.socket.as_ref() else {
            self.message = Some("No supervisor socket configured".to_string());
            return;
        };
        if evaluation.profile.curve.is_none() {
            self.message = Some(format!(
                "Profile unavailable: {}",
                evaluation.compatibility.label()
            ));
            return;
        }
        match Client::connect(socket) {
            Ok(mut client) => match client.call(
                "profile.select",
                serde_json::json!({"name": evaluation.profile.name}),
            ) {
                Ok(value) => {
                    let simulated = value
                        .get("hardware_applied")
                        .and_then(Value::as_bool)
                        .map(|applied| !applied)
                        .unwrap_or(false);
                    self.message = Some(if simulated {
                        format!(
                            "{} previewed; no fan write performed",
                            evaluation.profile.name
                        )
                    } else {
                        "Preview did not confirm simulation-only mode".to_string()
                    });
                    self.refresh();
                }
                Err(error) => self.message = Some(error.message),
            },
            Err(error) => self.message = Some(error.message),
        }
    }

    fn apply_preset(&mut self, index: usize) {
        let Some(preset) = presets::builtins().get(index).cloned() else {
            self.message = Some("Preset no longer available".to_string());
            return;
        };
        self.preset_index = index;
        if !self.supervisor.state.is_connected() {
            self.message = Some("Supervisor unavailable: TUI is read-only".to_string());
            return;
        }
        let Some(socket) = self.socket.as_ref() else {
            self.message = Some("No supervisor socket configured".to_string());
            return;
        };
        match Client::connect(socket) {
            Ok(mut client) => match client.call(
                "preset.apply",
                serde_json::json!({"name": preset.name, "confirm": true}),
            ) {
                Ok(value) => {
                    let applied = value
                        .get("hardware_applied")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    let lease_minutes =
                        crate::policy::SafetyConfig::default().manual_ttl_ms / 60_000;
                    self.message = Some(if applied {
                        format!(
                            "{} applied · CPU {:.0}% · GPU {:.0}% · automatic in {lease_minutes} min",
                            preset.title, preset.cpu_percent, preset.gpu_percent
                        )
                    } else {
                        value
                            .get("message")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| {
                                format!("{} preset was not hardware-applied", preset.title)
                            })
                    });
                    self.refresh();
                }
                Err(error) => self.message = Some(error.message),
            },
            Err(error) => self.message = Some(error.message),
        }
    }

    fn apply_auto(&mut self) {
        if !self.supervisor.state.is_connected() {
            self.message = Some("Supervisor unavailable: TUI is read-only".to_string());
            return;
        }
        let Some(socket) = self.socket.as_ref() else {
            self.message = Some("No supervisor socket configured".to_string());
            return;
        };
        match Client::connect(socket) {
            Ok(mut client) => match client.call("fan.auto", serde_json::json!({})) {
                Ok(value) => {
                    let verified = value
                        .get("verified")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    self.message = Some(
                        value
                            .get("message")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .unwrap_or_else(|| {
                                if verified {
                                    "Automatic mode confirmed by backend".to_string()
                                } else {
                                    "Automatic request not confirmed".to_string()
                                }
                            }),
                    );
                    self.refresh();
                }
                Err(error) => {
                    self.message = Some(error.message);
                    self.refresh();
                }
            },
            Err(error) => self.message = Some(error.message),
        }
    }

    fn render(&mut self, frame: &mut ratatui::Frame) {
        let area = frame.area();
        let header_height = header_height(area, self.ascii_logo);
        let [_top_margin, header, tabs, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(header_height),
            Constraint::Length(1),
            Constraint::Min(4),
            Constraint::Length(3),
        ])
        .areas(area);

        self.render_header(frame, header);
        self.render_tabs(frame, tabs);
        match self.tab {
            Tab::Dashboard => self.render_dashboard(frame, body),
            Tab::Sensors => self.render_sensors(frame, body),
            Tab::Fans => self.render_fans(frame, body),
            Tab::Profiles => self.render_profiles(frame, body),
            Tab::System => self.render_system(frame, body),
            Tab::Diagnostics => self.render_diagnostics(frame, body),
            Tab::Presets => self.render_presets(frame, body),
        }
        self.render_footer(frame, footer);
        if self.show_help {
            self.render_help(frame, area);
        }
    }

    fn header_dot(&self) -> &'static str {
        if self.ascii_logo { "* " } else { "● " }
    }

    fn header_rail(&self) -> &'static str {
        if self.ascii_logo { "| " } else { "│ " }
    }

    fn render_header_rule(&self, frame: &mut ratatui::Frame, area: Rect) {
        if self.ascii_logo {
            frame.render_widget(
                Paragraph::new("-".repeat(area.width as usize)).style(theme::muted()),
                area,
            );
        } else {
            frame.render_widget(
                Block::default()
                    .borders(Borders::BOTTOM)
                    .border_style(theme::muted()),
                area,
            );
        }
    }

    fn render_header(&self, frame: &mut ratatui::Frame, area: Rect) {
        if area.height >= 7 {
            self.render_banner_header(frame, area);
        } else if area.height == 3 {
            self.render_medium_header(frame, area);
        } else {
            self.render_compact_header(frame, area);
        }
    }

    fn render_banner_header(&self, frame: &mut ratatui::Frame, area: Rect) {
        let host = host_info();
        let (supervisor_color, supervisor_label) = supervisor_style(self.supervisor.state);
        let nbfc = self
            .view
            .version
            .map(|version| format!("NBFC {version}"))
            .unwrap_or_else(|| "NBFC unavailable".to_string());
        let logo: Vec<Line> = if self.ascii_logo {
            LOGO_BANNER_ASCII
                .iter()
                .map(|row| Line::from(Span::styled(format!(" {row}"), theme::heading())))
                .collect()
        } else {
            LOGO_BANNER
                .iter()
                .map(|row| Line::from(Span::styled(format!(" {row}"), theme::heading())))
                .collect()
        };
        let host_label = format!(
            "{} {}",
            host.vendor.as_deref().unwrap_or("Machine"),
            host.model.as_deref().unwrap_or("unknown")
        );

        if area.width >= 118 {
            let [content, rule] =
                Layout::vertical([Constraint::Length(6), Constraint::Length(1)]).areas(area);
            let [logo_area, meta_area] =
                Layout::horizontal([Constraint::Length(78), Constraint::Min(34)]).areas(content);
            frame.render_widget(Paragraph::new(logo), logo_area);
            let meta = vec![
                Line::from(vec![
                    Span::styled(self.header_rail(), theme::muted()),
                    Span::styled(crate::VERSION.to_string(), theme::heading()),
                ]),
                Line::from(vec![
                    Span::styled(self.header_rail(), theme::muted()),
                    Span::styled(TAGLINE, theme::muted()),
                ]),
                Line::from(vec![
                    Span::styled(self.header_rail(), theme::muted()),
                    Span::styled(self.header_dot(), Style::default().fg(supervisor_color)),
                    Span::styled(
                        format!("supervisor {supervisor_label}"),
                        Style::default().fg(supervisor_color),
                    ),
                ]),
                Line::from(vec![
                    Span::styled(self.header_rail(), theme::muted()),
                    Span::styled(nbfc, theme::muted()),
                ]),
                Line::from(vec![
                    Span::styled(self.header_rail(), theme::muted()),
                    Span::styled(host_label, theme::muted()),
                ]),
                Line::from(Span::styled(
                    if self.ascii_logo { "|" } else { "│" },
                    theme::muted(),
                )),
            ];
            frame.render_widget(Paragraph::new(meta), meta_area);
            self.render_header_rule(frame, rule);
            return;
        }

        let [content, status, rule] = Layout::vertical([
            Constraint::Length(6),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);
        frame.render_widget(Paragraph::new(logo), content);
        let status_line = Line::from(vec![
            Span::raw(" "),
            Span::styled(crate::VERSION.to_string(), theme::heading()),
            Span::styled("  ·  ", theme::muted()),
            Span::styled(TAGLINE, theme::muted()),
            Span::styled("  ·  ", theme::muted()),
            Span::styled(self.header_dot(), Style::default().fg(supervisor_color)),
            Span::styled(
                format!("supervisor {supervisor_label}"),
                Style::default().fg(supervisor_color),
            ),
            Span::styled(format!("  ·  {nbfc}"), theme::muted()),
        ]);
        frame.render_widget(Paragraph::new(status_line), status);
        self.render_header_rule(frame, rule);
    }

    fn render_medium_header(&self, frame: &mut ratatui::Frame, area: Rect) {
        let host = host_info();
        let (supervisor_color, supervisor_label) = supervisor_style(self.supervisor.state);
        let nbfc = self
            .view
            .version
            .map(|version| format!("NBFC {version}"))
            .unwrap_or_else(|| "NBFC unavailable".to_string());
        let [content, rule] =
            Layout::vertical([Constraint::Length(2), Constraint::Length(1)]).areas(area);
        let [logo_area, meta_area] =
            Layout::horizontal([Constraint::Length(36), Constraint::Min(10)]).areas(content);
        let logo: Vec<Line> = LOGO_MEDIUM
            .iter()
            .map(|row| Line::from(Span::styled(format!(" {row}"), theme::heading())))
            .collect();
        frame.render_widget(Paragraph::new(logo), logo_area);

        let compact_meta = area.width < 76;
        let supervisor_label = if compact_meta {
            supervisor_short(self.supervisor.state)
        } else {
            supervisor_label
        };
        let mut second = vec![Span::styled(nbfc, theme::muted())];
        if !compact_meta {
            second.push(Span::styled("  ·  ", theme::muted()));
            second.push(Span::styled(
                host.model.unwrap_or_else(|| "unknown machine".to_string()),
                theme::muted(),
            ));
        }
        let meta = vec![
            Line::from(vec![
                Span::styled(crate::VERSION.to_string(), theme::heading()),
                Span::styled("  ·  ", theme::muted()),
                Span::styled(self.header_dot(), Style::default().fg(supervisor_color)),
                Span::styled(
                    format!("supervisor {supervisor_label}"),
                    Style::default().fg(supervisor_color),
                ),
            ]),
            Line::from(second),
        ];
        frame.render_widget(Paragraph::new(meta), meta_area);
        self.render_header_rule(frame, rule);
    }

    fn render_compact_header(&self, frame: &mut ratatui::Frame, area: Rect) {
        let host = host_info();
        let (supervisor_color, supervisor_label) = supervisor_style(self.supervisor.state);
        let nbfc = self
            .view
            .version
            .map(|version| format!("NBFC {version}"))
            .unwrap_or_else(|| "NBFC unavailable".to_string());
        let compact = area.width < 100;
        let host_label = if compact {
            host.model
                .clone()
                .unwrap_or_else(|| "unknown machine".to_string())
        } else {
            format!(
                "{} {}",
                host.vendor.as_deref().unwrap_or("Machine"),
                host.model.as_deref().unwrap_or("unknown")
            )
        };
        let supervisor_label = if compact {
            supervisor_short(self.supervisor.state)
        } else {
            supervisor_label
        };

        let mut spans = vec![
            Span::styled(" NeoSense ", theme::heading()),
            Span::styled(crate::VERSION.to_string(), theme::muted()),
            Span::raw("  ·  "),
            Span::styled(host_label, Style::default().add_modifier(Modifier::BOLD)),
        ];
        if !compact {
            spans.push(Span::raw("  ·  "));
            spans.push(Span::styled(nbfc, theme::muted()));
        }
        spans.push(Span::raw("  ·  "));
        spans.push(Span::styled("● ", Style::default().fg(supervisor_color)));
        spans.push(Span::styled(
            format!("supervisor {supervisor_label}"),
            Style::default().fg(supervisor_color),
        ));

        let [content, rule] = Layout::vertical([
            Constraint::Length(area.height.saturating_sub(1)),
            Constraint::Length(1),
        ])
        .areas(area);
        frame.render_widget(Paragraph::new(Line::from(spans)), content);
        self.render_header_rule(frame, rule);
    }

    fn render_tabs(&self, frame: &mut ratatui::Frame, area: Rect) {
        if area.width < 62 {
            let line = Line::from(vec![
                Span::styled(self.tab.title(), theme::heading()),
                Span::styled(
                    format!("  ·  {}/{}  ·  Tab", self.tab.index() + 1, Tab::ALL.len()),
                    theme::muted(),
                ),
            ]);
            frame.render_widget(Paragraph::new(line), area);
            return;
        }
        let titles: Vec<Line> = Tab::ALL.iter().map(|tab| Line::from(tab.title())).collect();
        let tabs = Tabs::new(titles)
            .select(self.tab.index())
            .padding("", "")
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            )
            .divider(Span::styled(" · ", theme::muted()));
        frame.render_widget(tabs, area);
    }

    fn render_dashboard(&self, frame: &mut ratatui::Frame, area: Rect) {
        if area.width < 50 || area.height < 16 {
            self.render_dashboard_compact(frame, area);
            return;
        }
        let mode = self.operating_mode();
        let fan_count = self
            .view
            .status
            .as_ref()
            .map(|status| status.fans.len())
            .unwrap_or(0) as u16;
        let tall = area.height >= 24;
        let mode_height = if tall { 7 } else { 6 }.min(area.height.saturating_sub(4));
        let fans_height = (3 + fan_count)
            .max(4)
            .min(area.height.saturating_sub(mode_height + 3));
        let status_height = if tall { 9 } else { 5 };
        let [mode_area, fans_area, status_area, _spacer] = Layout::vertical([
            Constraint::Length(mode_height),
            Constraint::Length(fans_height),
            Constraint::Length(status_height),
            Constraint::Min(0),
        ])
        .areas(area);
        self.render_mode_panel(frame, mode_area, &mode);
        self.render_dashboard_fans(frame, fans_area);
        self.render_dashboard_status(frame, status_area);
    }

    fn render_mode_panel(&self, frame: &mut ratatui::Frame, area: Rect, mode: &OperatingMode) {
        let (tag, color) = mode.tag();
        let title = match mode {
            OperatingMode::Automatic => "Automatic (NBFC)".to_string(),
            OperatingMode::Preset(index) => presets::builtins()
                .get(*index)
                .map(|preset| format!("{} · fixed targets", preset.title))
                .unwrap_or_else(|| "Fixed targets".to_string()),
            OperatingMode::Profile(name) => {
                format!("Profile · {}", profile_display_name(name))
            }
            OperatingMode::Manual => "Manual targets".to_string(),
            OperatingMode::Failsafe => "Fail-safe".to_string(),
            OperatingMode::Degraded => "Degraded".to_string(),
            OperatingMode::Unknown => "State unavailable".to_string(),
        };
        let mut lines = vec![
            Line::from(vec![
                Span::styled("● ", Style::default().fg(color)),
                Span::styled(
                    title,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                mode_tag(tag, color),
            ]),
            Line::from(Span::styled(self.mode_detail(mode), theme::muted())),
            self.mode_selector(mode),
        ];
        if area.height >= 6 {
            lines.insert(2, Line::from(""));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .block(theme::panel_focused("Cooling mode"))
                .wrap(Wrap { trim: true }),
            area,
        );
    }

    fn mode_detail(&self, mode: &OperatingMode) -> String {
        let targets = self
            .backend_targets()
            .into_iter()
            .map(|(name, target)| {
                let label = name
                    .split_whitespace()
                    .next()
                    .unwrap_or("fan")
                    .to_ascii_uppercase();
                format!("{label} {target}")
            })
            .collect::<Vec<_>>();
        let targets = if targets.is_empty() {
            "targets unavailable".to_string()
        } else {
            targets.join("  ·  ")
        };
        match mode {
            OperatingMode::Automatic => {
                format!("{targets}  ·  NBFC temperature curve  ·  no manual lease")
            }
            OperatingMode::Preset(_) => {
                let lease = self
                    .safety_doc()
                    .and_then(|safety| safety.get("preset"))
                    .and_then(Value::as_array)
                    .and_then(|leases| leases.first())
                    .and_then(|lease| lease.get("expires_at_ms"))
                    .and_then(Value::as_u64)
                    .map(|expires| {
                        let remaining = expires.saturating_sub(now_ms()) / 1000;
                        if remaining >= 60 {
                            format!("auto in {}m{:02}s", remaining / 60, remaining % 60)
                        } else {
                            format!("auto in {remaining}s")
                        }
                    })
                    .unwrap_or_else(|| "lease active".to_string());
                format!("{targets}  ·  fixed percentage  ·  {lease}")
            }
            OperatingMode::Profile(_) => {
                let profile = self.profile_doc();
                let applied = fan_index_list(profile.and_then(|p| p.get("applied_fans")));
                let blocked = fan_index_list(profile.and_then(|p| p.get("blocked_fans")));
                format!("{targets}  ·  closed loop  ·  applied {applied}  ·  blocked {blocked}")
            }
            OperatingMode::Manual => {
                format!(
                    "CPU {targets}  ·  manual percentage  ·  automatic recovery on lease expiry"
                )
            }
            OperatingMode::Failsafe | OperatingMode::Degraded => {
                let reasons = self.safety_reasons();
                if reasons.is_empty() {
                    "Supervisor is not in a normal automatic state".to_string()
                } else {
                    reasons
                }
            }
            OperatingMode::Unknown => {
                "Connect the supervisor to read the current cooling mode".to_string()
            }
        }
    }

    fn mode_selector(&self, mode: &OperatingMode) -> Line<'static> {
        let mut spans = Vec::new();
        let keys = ["s", "b", "m"];
        for (index, preset) in presets::builtins().iter().enumerate() {
            if index > 0 {
                spans.push(Span::raw("   "));
            }
            let active = mode.preset_index() == Some(index);
            if active {
                spans.push(Span::styled("● ", theme::active()));
                spans.push(Span::styled(
                    format!(
                        "[{}] {} {:.0}%",
                        keys.get(index).copied().unwrap_or("?"),
                        preset.title,
                        preset.cpu_percent
                    ),
                    theme::active(),
                ));
            } else {
                spans.push(Span::styled(
                    format!("[{}] ", keys.get(index).copied().unwrap_or("?")),
                    theme::muted(),
                ));
                spans.push(Span::raw(format!(
                    "{} {:.0}%",
                    preset.title, preset.cpu_percent
                )));
            }
        }
        spans.push(Span::raw("   "));
        if matches!(mode, OperatingMode::Automatic) {
            spans.push(Span::styled("● ", theme::active()));
            spans.push(Span::styled("[a] Auto", theme::active()));
        } else {
            spans.push(Span::styled("[a] ", theme::muted()));
            spans.push(Span::raw("Auto"));
        }
        Line::from(spans)
    }

    fn render_dashboard_fans(&self, frame: &mut ratatui::Frame, area: Rect) {
        let compact = area.width < 76;
        let Some(inventory) = self.fan_inventory() else {
            let reason = crate::fans::build(&self.view)
                .err()
                .unwrap_or_else(|| "fan discovery failed".to_string());
            frame.render_widget(
                Paragraph::new(format!("Fan state unavailable: {reason}"))
                    .block(theme::panel("Fans"))
                    .wrap(Wrap { trim: true }),
                area,
            );
            return;
        };
        let rows = inventory.fans.iter().map(|fan| {
            let (mode_label, mode_color) = match fan.mode.as_str() {
                "automatic" => ("auto", theme::ACTIVE),
                "manual" => ("manual", theme::WARN),
                _ => ("unknown", theme::MUTED),
            };
            let target = opt_percent(fan.target_percent);
            let ec = opt_percent(fan.reported_percent);
            let sensor = if fan.temperature_sources.is_empty() {
                "none".to_string()
            } else {
                fan.temperature_sources.join(",")
            };
            if compact {
                Row::new(vec![
                    Cell::from(fan.display_name.clone()),
                    Cell::from(Span::styled(mode_label, Style::default().fg(mode_color))),
                    Cell::from(target),
                    Cell::from(ec),
                ])
            } else {
                Row::new(vec![
                    Cell::from(fan.display_name.clone()),
                    Cell::from(fan.role.to_string()),
                    Cell::from(Span::styled(mode_label, Style::default().fg(mode_color))),
                    Cell::from(target),
                    Cell::from(ec),
                    Cell::from(sensor),
                ])
            }
        });
        let (header, widths) = if compact {
            (
                Row::new(["Fan", "Mode", "Target", "EC"]),
                vec![
                    Constraint::Min(12),
                    Constraint::Length(7),
                    Constraint::Length(8),
                    Constraint::Length(8),
                ],
            )
        } else {
            (
                Row::new(["Fan", "Role", "Mode", "Target", "EC readback", "Sensor"]),
                vec![
                    Constraint::Min(12),
                    Constraint::Length(6),
                    Constraint::Length(7),
                    Constraint::Length(8),
                    Constraint::Length(12),
                    Constraint::Min(8),
                ],
            )
        };
        let table = Table::new(rows, widths).column_spacing(2).header(
            header.style(
                Style::default()
                    .fg(theme::MUTED)
                    .add_modifier(Modifier::BOLD),
            ),
        );
        frame.render_widget(table.block(theme::panel("Fans")), area);
    }

    fn render_dashboard_status(&self, frame: &mut ratatui::Frame, area: Rect) {
        let now = now_ms();
        let cpu = self
            .view
            .snapshot
            .cpu_package(now, 5_000)
            .map(|sample| sample.display_value())
            .unwrap_or_else(|| "unavailable".to_string());
        let gpu = self
            .view
            .snapshot
            .gpu_indexes()
            .first()
            .and_then(|index| self.view.snapshot.gpu_die(*index, now, 5_000))
            .map(|sample| sample.display_value())
            .unwrap_or_else(|| "unavailable".to_string());
        let safety = self
            .safety_doc()
            .and_then(|safety| safety.get("state"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let safety_color = match safety {
            "safe_auto" => theme::ACTIVE,
            "profile" => theme::ACCENT,
            "manual" => theme::WARN,
            "failsafe" => theme::ERROR,
            _ => theme::MUTED,
        };
        let warnings = warnings_from_view(&self.view, Some(&self.supervisor));
        let mut lines = vec![Line::from(vec![
            Span::styled("CPU ", theme::muted()),
            Span::styled(cpu, Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("     "),
            Span::styled("GPU ", theme::muted()),
            Span::styled(gpu, Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("     "),
            Span::styled("Safety ", theme::muted()),
            Span::styled(safety, Style::default().fg(safety_color)),
        ])];
        let reasons = self.safety_reasons();
        if !reasons.is_empty() {
            lines.push(Line::from(Span::styled(
                truncate_for_width(&reasons, area.width.saturating_sub(4) as usize),
                Style::default().fg(theme::ERROR),
            )));
        }
        if warnings.is_empty() {
            lines.push(Line::from(Span::styled(
                "✓ no active warnings",
                Style::default().fg(theme::ACTIVE),
            )));
        } else {
            let warning_limit = if area.height >= 8 { 4 } else { 2 };
            for warning in warnings.iter().take(warning_limit) {
                let color = match warning.severity {
                    Severity::Critical => theme::ERROR,
                    Severity::Warning => theme::WARN,
                    Severity::Info => theme::ACCENT,
                    Severity::Success => theme::ACTIVE,
                };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{} ", warning.severity.label()),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        truncate_for_width(&warning.title, area.width.saturating_sub(8) as usize),
                        Style::default().fg(color),
                    ),
                ]));
            }
        }
        if area.height >= 6 {
            lines.push(Line::from(Span::styled(
                "Targets are NBFC percentages; EC readback is not measured RPM.",
                theme::muted(),
            )));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .block(theme::panel("Status"))
                .wrap(Wrap { trim: true }),
            area,
        );
    }

    fn render_dashboard_compact(&self, frame: &mut ratatui::Frame, area: Rect) {
        let mode = self.operating_mode();
        let (tag, color) = mode.tag();
        let title = match &mode {
            OperatingMode::Automatic => "Automatic (NBFC)".to_string(),
            OperatingMode::Preset(index) => presets::builtins()
                .get(*index)
                .map(|preset| preset.title.to_string())
                .unwrap_or_else(|| "Fixed targets".to_string()),
            OperatingMode::Profile(name) => profile_display_name(name),
            OperatingMode::Manual => "Manual targets".to_string(),
            OperatingMode::Failsafe => "Fail-safe".to_string(),
            OperatingMode::Degraded => "Degraded".to_string(),
            OperatingMode::Unknown => "Unavailable".to_string(),
        };
        let mut lines = vec![
            Line::from(vec![
                Span::styled("● ", Style::default().fg(color)),
                Span::styled(
                    title,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                mode_tag(tag, color),
            ]),
            Line::from(Span::styled(self.mode_detail(&mode), theme::muted())),
            self.mode_selector(&mode),
        ];
        if area.height >= 5 {
            lines.push(self.dashboard_temperature_line());
        }
        frame.render_widget(
            Paragraph::new(lines)
                .block(theme::panel("Cooling mode"))
                .wrap(Wrap { trim: true }),
            area,
        );
    }

    fn dashboard_temperature_line(&self) -> Line<'static> {
        let now = now_ms();
        let cpu = self
            .view
            .snapshot
            .cpu_package(now, 5_000)
            .map(|sample| sample.display_value())
            .unwrap_or_else(|| "unavailable".to_string());
        let gpu = self
            .view
            .snapshot
            .gpu_indexes()
            .first()
            .and_then(|index| self.view.snapshot.gpu_die(*index, now, 5_000))
            .map(|sample| sample.display_value())
            .unwrap_or_else(|| "unavailable".to_string());
        let safety = self
            .safety_doc()
            .and_then(|safety| safety.get("state"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        Line::from(vec![
            Span::styled("CPU ", theme::muted()),
            Span::styled(cpu, Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("   "),
            Span::styled("GPU ", theme::muted()),
            Span::styled(gpu, Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("   "),
            Span::styled("Safety ", theme::muted()),
            Span::raw(safety.to_string()),
        ])
    }

    fn render_sensors(&self, frame: &mut ratatui::Frame, area: Rect) {
        if self.view.snapshot.sensors.is_empty() {
            frame.render_widget(
                Paragraph::new(
                    "No sensors detected. Check hwmon/thermal access and run `neosense diagnose`.",
                )
                .block(theme::panel("Sensors"))
                .wrap(Wrap { trim: true }),
                area,
            );
            return;
        }
        let compact = area.width < 76;
        let rows = self.view.snapshot.sensors.iter().map(|sensor| {
            let trust_style = Style::default().fg(trust_color(sensor.health.trust));
            if compact {
                Row::new(vec![
                    Span::raw(sensor.role.to_string()),
                    Span::styled(
                        sensor.display_value(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(sensor.health.trust.short_label(), trust_style),
                ])
            } else {
                Row::new(vec![
                    Span::raw(sensor.role.to_string()),
                    Span::styled(
                        sensor.display_value(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(sensor.health.trust.short_label(), trust_style),
                    Span::styled(format!("{:?}", sensor.status), theme::muted()),
                    Span::styled(sensor.provider.to_string(), theme::muted()),
                    Span::styled(
                        sensor
                            .path
                            .as_deref()
                            .or(sensor.label.as_deref())
                            .unwrap_or("—")
                            .to_string(),
                        theme::muted(),
                    ),
                ])
            }
        });
        let (header, widths) = if compact {
            (
                Row::new(["Role", "Value", "Trust"]),
                vec![
                    Constraint::Length(16),
                    Constraint::Length(12),
                    Constraint::Min(8),
                ],
            )
        } else {
            (
                Row::new(["Role", "Value", "Trust", "Status", "Provider", "Path"]),
                vec![
                    Constraint::Length(18),
                    Constraint::Length(12),
                    Constraint::Length(11),
                    Constraint::Length(10),
                    Constraint::Length(10),
                    Constraint::Min(16),
                ],
            )
        };
        let table = Table::new(rows, widths).column_spacing(2).header(
            header.style(
                Style::default()
                    .fg(theme::MUTED)
                    .add_modifier(Modifier::BOLD),
            ),
        );
        let mut state = TableState::default().with_offset(self.scroll as usize);
        frame.render_stateful_widget(
            table.block(theme::panel(format!(
                "Sensors · {} discovered",
                self.view.snapshot.sensors.len()
            ))),
            area,
            &mut state,
        );
    }

    fn render_fans(&self, frame: &mut ratatui::Frame, area: Rect) {
        let Some(inventory) = self.fan_inventory() else {
            let reason = crate::fans::build(&self.view)
                .err()
                .unwrap_or_else(|| "fan discovery failed".to_string());
            frame.render_widget(
                Paragraph::new(format!(
                    "Fan discovery unavailable: {reason}. The TUI never reads hardware directly."
                ))
                .block(theme::panel("Fan Control"))
                .wrap(Wrap { trim: true }),
                area,
            );
            return;
        };
        if inventory.fans.is_empty() {
            frame.render_widget(
                Paragraph::new("No logical fans were discovered from the backend.")
                    .block(theme::panel("Fan Control"))
                    .wrap(Wrap { trim: true }),
                area,
            );
            return;
        }

        let both = inventory.fans.len() >= 2;
        let row_count = inventory.fans.len() + usize::from(both);
        let selected = self.selected_fan.min(row_count.saturating_sub(1));
        let compact = area.width < 88;
        let show_detail = area.height >= 16;

        let mut rows = Vec::new();
        for (row_index, fan) in inventory.fans.iter().enumerate() {
            let is_selected = row_index == selected;
            let marker = if is_selected { "▶" } else { " " };
            let (mode_label, mode_color) = match fan.mode.as_str() {
                "automatic" => ("auto", theme::ACTIVE),
                "manual" => ("manual", theme::WARN),
                _ => ("unknown", theme::MUTED),
            };
            let temperature = fan
                .temperature
                .as_ref()
                .map(|value| format!("{:.0}°C", value.value_c))
                .unwrap_or_else(|| "—".into());
            let rpm = fan
                .measured_rpm
                .as_ref()
                .and_then(|rpm| rpm.value)
                .map(|value| format!("{value:.0}"))
                .unwrap_or_else(|| "—".into());
            let cells = if compact {
                vec![
                    Cell::from(Span::styled(marker, Style::default().fg(theme::ACCENT))),
                    Cell::from(fan.display_name.clone()),
                    Cell::from(Span::styled(mode_label, Style::default().fg(mode_color))),
                    Cell::from(opt_percent(fan.target_percent)),
                    Cell::from(temperature),
                ]
            } else {
                vec![
                    Cell::from(Span::styled(marker, Style::default().fg(theme::ACCENT))),
                    Cell::from(fan.display_name.clone()),
                    Cell::from(fan.role.to_string()),
                    Cell::from(Span::styled(mode_label, Style::default().fg(mode_color))),
                    Cell::from(opt_percent(fan.target_percent)),
                    Cell::from(opt_percent(fan.reported_percent)),
                    Cell::from(temperature),
                    Cell::from(rpm),
                ]
            };
            let style = if is_selected {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            rows.push(Row::new(cells).style(style));
        }
        if both {
            let is_selected = selected >= inventory.fans.len();
            let marker = if is_selected { "▶" } else { " " };
            let auto_all = inventory
                .fans
                .iter()
                .all(|fan| fan.mode.eq_ignore_ascii_case("automatic"));
            let (mode_label, mode_color) = if auto_all {
                ("auto", theme::ACTIVE)
            } else {
                ("manual", theme::WARN)
            };
            let targets: Vec<f64> = inventory
                .fans
                .iter()
                .filter_map(|fan| fan.target_percent)
                .collect();
            let common_target = targets
                .first()
                .copied()
                .filter(|first| targets.iter().all(|value| (value - first).abs() < 0.5))
                .map(|value| format!("{value:.1}%"))
                .unwrap_or_else(|| "mixed".to_string());
            let cells = if compact {
                vec![
                    Cell::from(Span::styled(marker, Style::default().fg(theme::ACCENT))),
                    Cell::from("Both fans"),
                    Cell::from(Span::styled(mode_label, Style::default().fg(mode_color))),
                    Cell::from(common_target),
                    Cell::from("—"),
                ]
            } else {
                vec![
                    Cell::from(Span::styled(marker, Style::default().fg(theme::ACCENT))),
                    Cell::from("Both fans"),
                    Cell::from("both"),
                    Cell::from(Span::styled(mode_label, Style::default().fg(mode_color))),
                    Cell::from(common_target),
                    Cell::from("—"),
                    Cell::from("—"),
                    Cell::from("—"),
                ]
            };
            let style = if is_selected {
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            rows.push(Row::new(cells).style(style));
        }

        let (header, widths) = if compact {
            (
                Row::new(["", "Fan", "Mode", "Target", "Temp"]),
                vec![
                    Constraint::Length(2),
                    Constraint::Min(12),
                    Constraint::Length(7),
                    Constraint::Length(8),
                    Constraint::Length(6),
                ],
            )
        } else {
            (
                Row::new(["", "Fan", "Role", "Mode", "Target", "EC", "Temp", "RPM"]),
                vec![
                    Constraint::Length(2),
                    Constraint::Min(12),
                    Constraint::Length(6),
                    Constraint::Length(7),
                    Constraint::Length(8),
                    Constraint::Length(8),
                    Constraint::Length(6),
                    Constraint::Length(6),
                ],
            )
        };
        let (table_area, detail_area) = if show_detail {
            let [table, detail] =
                Layout::vertical([Constraint::Min(5), Constraint::Length(8)]).areas(area);
            (table, Some(detail))
        } else {
            (area, None)
        };
        let table = Table::new(rows, widths).column_spacing(2).header(
            header.style(
                Style::default()
                    .fg(theme::MUTED)
                    .add_modifier(Modifier::BOLD),
            ),
        );
        frame.render_widget(table.block(theme::panel_focused("Fans")), table_area);
        if let Some(detail_area) = detail_area {
            self.render_fan_detail(frame, detail_area, &inventory, selected);
        }
    }

    fn render_fan_detail(
        &self,
        frame: &mut ratatui::Frame,
        area: Rect,
        inventory: &crate::fans::FanInventory,
        selected: usize,
    ) {
        if selected >= inventory.fans.len() {
            let targets: Vec<String> = inventory
                .fans
                .iter()
                .map(|fan| format!("{} {}", fan.display_name, opt_percent(fan.target_percent)))
                .collect();
            let lines = vec![
                Line::from(Span::styled(
                    "One target for every logical fan",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(targets.join("  ·  "), theme::muted())),
                Line::from("m or b set both  ·  +/− adjust 5%  ·  A all automatic"),
            ];
            frame.render_widget(
                Paragraph::new(lines)
                    .block(theme::panel_focused("Selected · Both fans"))
                    .wrap(Wrap { trim: true }),
                area,
            );
            return;
        }
        let fan = &inventory.fans[selected];
        let temperature = fan
            .temperature
            .as_ref()
            .map(|value| {
                format!(
                    "{:.1}°C {} {}",
                    value.value_c,
                    value.trust.short_label(),
                    if value.fresh { "fresh" } else { "stale" }
                )
            })
            .unwrap_or_else(|| "unavailable".into());
        let rpm = fan
            .measured_rpm
            .as_ref()
            .and_then(|reading| {
                reading
                    .value
                    .map(|value| format!("{value:.0} {}", reading.unit))
            })
            .unwrap_or_else(|| "unavailable".into());
        let target_percent = fan.target_percent.unwrap_or(0.0);
        let bar_width: usize = if area.width < 100 { 14 } else { 22 };
        let mut target_line = vec![Span::styled("Target ", theme::muted())];
        target_line.push(Span::styled(
            opt_percent(fan.target_percent),
            Style::default().add_modifier(Modifier::BOLD),
        ));
        target_line.push(Span::raw("  "));
        target_line.extend(percent_bar(target_percent, bar_width));
        target_line.push(Span::styled(
            format!(
                "  requested {}  ·  EC {}",
                opt_percent(fan.requested_target_percent),
                opt_percent(fan.reported_percent)
            ),
            theme::muted(),
        ));

        let lease = self.lease_label(fan.index).unwrap_or_else(|| {
            if fan.mode == "automatic" {
                "no manual lease".to_string()
            } else {
                "lease not recorded".to_string()
            }
        });
        let mut lines = vec![
            Line::from(target_line),
            Line::from(vec![
                Span::styled("Mode ", theme::muted()),
                Span::raw(fan.mode.clone()),
                Span::styled("  ·  source ", theme::muted()),
                Span::raw(if fan.temperature_sources.is_empty() {
                    "none".to_string()
                } else {
                    fan.temperature_sources.join(",")
                }),
                Span::styled("  ·  temp ", theme::muted()),
                Span::raw(temperature),
            ]),
            Line::from(vec![
                Span::styled("RPM ", theme::muted()),
                Span::raw(rpm),
                Span::styled("  ·  ", theme::muted()),
                Span::styled(lease, theme::muted()),
            ]),
            Line::from(Span::styled(
                "m set target  ·  +/− adjust 5%  ·  a automatic  ·  A all automatic",
                theme::muted(),
            )),
        ];
        for reason in fan.manual_control.reasons.iter().take(2) {
            lines.push(Line::from(Span::styled(
                format!("Manual blocked: {reason}"),
                Style::default().fg(theme::WARN),
            )));
        }
        for warning in fan.warnings.iter().take(1) {
            lines.push(Line::from(Span::styled(
                warning.clone(),
                Style::default().fg(theme::WARN),
            )));
        }
        if let Some((index, buffer)) = &self.editing_fan
            && *index == fan.index
        {
            lines.push(Line::from(Span::styled(
                format!("Target input: {buffer}_"),
                Style::default().fg(theme::ACTIVE),
            )));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .block(theme::panel_focused(format!(
                    "Selected · {}",
                    fan.display_name
                )))
                .wrap(Wrap { trim: true }),
            area,
        );
    }

    fn render_profiles(&self, frame: &mut ratatui::Frame, area: Rect) {
        let compact = area.width < 88;
        let evaluations = self.evaluations();
        let selected = self.profile_index.min(evaluations.len().saturating_sub(1));
        let active = self.operating_mode().profile_name().map(str::to_string);
        let profile_doc = self.profile_doc();
        let failed = profile_doc
            .filter(|profile| profile.get("active").and_then(Value::as_bool) != Some(true))
            .and_then(|profile| profile.get("selected"))
            .and_then(Value::as_str)
            .filter(|_name| {
                profile_doc
                    .and_then(|profile| profile.get("reason"))
                    .and_then(Value::as_str)
                    .is_some_and(|reason| !reason.contains("simulation"))
            })
            .map(str::to_string);
        let observations = fan_observations(&self.view);
        let rows = evaluations.iter().enumerate().map(|(index, evaluation)| {
            let is_active = active.as_deref() == Some(evaluation.profile.name.as_str());
            let is_selected = index == selected;
            let marker = if is_active {
                "●"
            } else if is_selected {
                "▶"
            } else {
                " "
            };
            let style = if is_active {
                Style::default()
                    .fg(theme::ACTIVE)
                    .add_modifier(Modifier::BOLD)
            } else if is_selected {
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let availability_color = if evaluation.compatibility.can_apply() {
                theme::ACTIVE
            } else {
                theme::WARN
            };
            let target = evaluation
                .profile
                .curve
                .as_ref()
                .and_then(|curve| {
                    let observations: Vec<_> = observations
                        .iter()
                        .filter(|fan| {
                            evaluation
                                .profile
                                .fan_roles
                                .as_ref()
                                .is_none_or(|roles| roles.contains(&fan.role))
                        })
                        .cloned()
                        .collect();
                    evaluate_profile(
                        curve,
                        &self.view.snapshot,
                        &observations,
                        RegulationMode::Preview,
                        now_ms(),
                    )
                    .ok()
                })
                .and_then(|targets| targets.into_iter().find(|target| target.available))
                .and_then(|target| target.target_percent)
                .map(|percent| format!("{percent:.1}%"))
                .unwrap_or_else(|| "n/a".to_string());
            let origin = match evaluation.profile.origin {
                ProfileOrigin::Builtin => "built-in",
                ProfileOrigin::Custom => "custom",
            };
            if compact {
                Row::new(vec![
                    Cell::from(Span::styled(marker, style)),
                    Cell::from(evaluation.profile.name.clone()),
                    Cell::from(Span::styled(
                        evaluation.compatibility.short_label().to_string(),
                        Style::default().fg(availability_color),
                    )),
                    Cell::from(target),
                ])
                .style(style)
            } else {
                Row::new(vec![
                    Cell::from(Span::styled(marker, style)),
                    Cell::from(evaluation.profile.name.clone()),
                    Cell::from(origin),
                    Cell::from(evaluation.profile.title.clone()),
                    Cell::from(Span::styled(
                        evaluation.compatibility.short_label().to_string(),
                        Style::default().fg(availability_color),
                    )),
                    Cell::from(target),
                ])
                .style(style)
            }
        });
        let (header, widths) = if compact {
            (
                Row::new(["", "Profile", "Availability", "Target"]),
                vec![
                    Constraint::Length(2),
                    Constraint::Min(12),
                    Constraint::Length(12),
                    Constraint::Length(8),
                ],
            )
        } else {
            (
                Row::new(["", "Profile", "Origin", "Title", "Availability", "Target"]),
                vec![
                    Constraint::Length(2),
                    Constraint::Min(12),
                    Constraint::Length(9),
                    Constraint::Min(12),
                    Constraint::Length(12),
                    Constraint::Length(8),
                ],
            )
        };
        let (table_area, detail_area) = if area.height >= 20 {
            let [table, detail] =
                Layout::vertical([Constraint::Min(6), Constraint::Length(11)]).areas(area);
            (table, Some(detail))
        } else {
            (area, None)
        };
        let block = if let Some(name) = &active {
            theme::panel_colored(
                format!("Profiles · ● {} active", profile_display_name(name)),
                theme::ACTIVE,
            )
        } else if let Some(name) = &failed {
            theme::panel_colored(
                format!("Profiles · {} inactive", profile_display_name(name)),
                theme::WARN,
            )
        } else {
            theme::panel_focused("Profiles")
        };
        let table = Table::new(rows, widths).column_spacing(2).header(
            header.style(
                Style::default()
                    .fg(theme::MUTED)
                    .add_modifier(Modifier::BOLD),
            ),
        );
        let visible_rows = table_area.height.saturating_sub(3) as usize;
        let offset = selected.saturating_sub(visible_rows.saturating_sub(1));
        let mut state = TableState::default().with_offset(offset);
        frame.render_stateful_widget(table.block(block), table_area, &mut state);
        if let Some(detail_area) = detail_area
            && let Some(evaluation) = evaluations.get(selected)
        {
            let bar_width: usize = if detail_area.width < 96 { 14 } else { 24 };
            let mut lines = Vec::new();
            if let Some(curve) = &evaluation.profile.curve {
                for point in curve.points.iter().take(5) {
                    let mut line = vec![Span::styled(
                        format!("{:>3.0}°C  {:>3.0}%  ", point.temperature_c, point.percent),
                        Style::default(),
                    )];
                    line.extend(percent_bar(point.percent, bar_width));
                    lines.push(Line::from(line));
                }
                lines.push(Line::from(Span::styled(
                    format!(
                        "floor {}% · ceiling {}% · hysteresis {}°C · max step {}%",
                        curve.min_percent,
                        curve.max_percent,
                        curve.hysteresis_c,
                        curve.max_step_percent
                    ),
                    theme::muted(),
                )));
            } else {
                lines.push(Line::from(
                    "No curve: this profile restores NBFC automatic mode.",
                ));
            }
            lines.push(Line::from(vec![
                Span::styled("Eligible fans ", theme::muted()),
                Span::raw(format!("{:?}", evaluation.eligible_fans)),
                Span::styled("  ·  blocked ", theme::muted()),
                Span::styled(
                    format!("{:?}", evaluation.blocked_fans),
                    if evaluation.blocked_fans.is_empty() {
                        theme::muted()
                    } else {
                        Style::default().fg(theme::WARN)
                    },
                ),
                Span::styled(
                    if evaluation.compatibility.can_apply() {
                        "  ·  ready to apply"
                    } else {
                        "  ·  blocked by policy"
                    },
                    theme::muted(),
                ),
            ]));
            for reason in evaluation.reasons.iter().take(1) {
                lines.push(Line::from(Span::styled(
                    reason.clone(),
                    Style::default().fg(theme::WARN),
                )));
            }
            lines.push(Line::from(Span::styled(
                "Enter apply  ·  p preview  ·  d deactivate  ·  e edit  ·  n new",
                theme::muted(),
            )));
            frame.render_widget(
                Paragraph::new(lines)
                    .block(theme::panel_focused(format!(
                        "Selected · {} · {}",
                        evaluation.profile.name,
                        evaluation.compatibility.label()
                    )))
                    .wrap(Wrap { trim: true }),
                detail_area,
            );
        }
    }

    fn render_presets(&self, frame: &mut ratatui::Frame, area: Rect) {
        let presets = presets::builtins();
        let selected = self.preset_index.min(presets.len().saturating_sub(1));
        let active = self.operating_mode().preset_index();
        if area.width >= 96 && area.height >= 16 {
            let [cards_area, note_area, _spacer] = Layout::vertical([
                Constraint::Length(10),
                Constraint::Length(6),
                Constraint::Min(0),
            ])
            .areas(area);
            let columns = Layout::horizontal([
                Constraint::Ratio(1, 3),
                Constraint::Ratio(1, 3),
                Constraint::Ratio(1, 3),
            ])
            .spacing(1)
            .split(cards_area);
            for (index, preset) in presets.iter().enumerate() {
                let is_active = active == Some(index);
                let is_selected = index == selected;
                let block = if is_active {
                    theme::panel_colored(format!(" ● {}", preset.title), theme::ACTIVE)
                } else if is_selected {
                    theme::panel_focused(format!("▶ {}", preset.title))
                } else {
                    theme::panel(preset.title.to_string())
                };
                let mut lines = Vec::new();
                if is_active {
                    lines.push(Line::from(Span::styled(
                        "● ACTIVE",
                        Style::default()
                            .fg(theme::ACTIVE)
                            .add_modifier(Modifier::BOLD),
                    )));
                } else if is_selected {
                    lines.push(Line::from(Span::styled(
                        "▶ SELECTED",
                        Style::default()
                            .fg(theme::ACCENT)
                            .add_modifier(Modifier::BOLD),
                    )));
                } else {
                    lines.push(Line::from(Span::styled("available", theme::muted())));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{:>3.0}%", preset.cpu_percent),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  CPU", theme::muted()),
                ]));
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("{:>3.0}%", preset.gpu_percent),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("  GPU", theme::muted()),
                ]));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled("fixed target", theme::muted())));
                frame.render_widget(
                    Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
                    columns[index],
                );
            }
            let selected_preset = presets[selected].clone();
            let note = vec![
                Line::from(vec![
                    Span::styled("Selected mode  ", theme::muted()),
                    Span::styled(
                        selected_preset.title,
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(selected_preset.description),
                Line::from(
                    "Enter applies immediately. A fixed target lasts 15 minutes, then the supervisor restores the NBFC automatic curve.",
                ),
                Line::from("Auto is always available and hands both fans back to NBFC."),
                Line::from(Span::styled(
                    "Percentages are backend targets, not measured RPM.",
                    theme::muted(),
                )),
            ];
            frame.render_widget(
                Paragraph::new(note)
                    .block(theme::panel("Presets"))
                    .wrap(Wrap { trim: true }),
                note_area,
            );
            return;
        }

        let rows = presets.iter().enumerate().map(|(index, preset)| {
            let is_active = active == Some(index);
            let is_selected = index == selected;
            let marker = if is_active {
                "●"
            } else if is_selected {
                "▶"
            } else {
                " "
            };
            let style = if is_active {
                Style::default()
                    .fg(theme::ACTIVE)
                    .add_modifier(Modifier::BOLD)
            } else if is_selected {
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Row::new(vec![
                Span::styled(marker, style),
                Span::styled(preset.title.to_string(), style),
                Span::raw(format!("{:.0}%", preset.cpu_percent)),
                Span::raw(format!("{:.0}%", preset.gpu_percent)),
                Span::styled(if is_active { "active" } else { "fixed" }, theme::muted()),
            ])
            .style(style)
        });
        let table = Table::new(
            rows,
            [
                Constraint::Length(2),
                Constraint::Min(12),
                Constraint::Length(7),
                Constraint::Length(7),
                Constraint::Length(8),
            ],
        )
        .column_spacing(2)
        .header(
            Row::new(["", "Mode", "CPU", "GPU", "State"]).style(
                Style::default()
                    .fg(theme::MUTED)
                    .add_modifier(Modifier::BOLD),
            ),
        );
        frame.render_widget(table.block(theme::panel_focused("Presets")), area);
    }

    fn render_system(&self, frame: &mut ratatui::Frame, area: Rect) {
        let mut lines = Vec::new();
        push_section_header(&mut lines, "Services");
        lines.push(Line::from(vec![
            Span::styled("NBFC service   ", theme::muted()),
            Span::raw(format!(
                "{} / {}",
                self.view
                    .service
                    .active_state
                    .as_deref()
                    .unwrap_or("unknown"),
                self.view
                    .service
                    .unit_file_state
                    .as_deref()
                    .unwrap_or("unknown")
            )),
        ]));
        if let Some(error) = &self.view.service.error {
            lines.push(Line::from(Span::styled(
                format!("Service error: {error}"),
                Style::default().fg(theme::ERROR),
            )));
        }
        if let Some(error) = &self.view.backend_error {
            lines.push(Line::from(Span::styled(
                format!("Backend error: {error}"),
                Style::default().fg(theme::ERROR),
            )));
        }
        lines.push(Line::from(vec![
            Span::styled("Supervisor     ", theme::muted()),
            Span::styled(
                self.supervisor.state.label().to_string(),
                Style::default().fg(supervisor_style(self.supervisor.state).0),
            ),
        ]));
        if let Some(health) = self.supervisor.health {
            lines.push(Line::from(vec![
                Span::styled("Health         ", theme::muted()),
                Span::styled(
                    if health { "healthy" } else { "degraded" },
                    Style::default().fg(if health { theme::ACTIVE } else { theme::WARN }),
                ),
            ]));
        }
        if let Some(reason) = &self.supervisor.reason {
            lines.push(Line::from(vec![
                Span::styled("Reason         ", theme::muted()),
                Span::raw(reason.clone()),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("IPC socket     ", theme::muted()),
            Span::raw(format!(
                "{}  ·  mode {}  ·  uid {}  ·  gid {}",
                self.supervisor.socket.display(),
                self.supervisor.socket_mode.as_deref().unwrap_or("n/a"),
                self.supervisor
                    .socket_uid
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "n/a".into()),
                self.supervisor
                    .socket_gid
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "n/a".into())
            )),
        ]));

        push_section_header(&mut lines, "Backend");
        if let Some(runtime) = &self.view.nbfc_runtime {
            lines.push(Line::from(vec![
                Span::styled("EC backend     ", theme::muted()),
                Span::raw(format!(
                    "{}  ·  pid {}",
                    runtime.summary(),
                    runtime
                        .pid
                        .map(|pid| pid.to_string())
                        .unwrap_or_else(|| "unknown".to_string())
                )),
            ]));
        }
        if let Some(status) = self.view.status.as_ref()
            && let Some(selected) = status.selected_config.as_deref()
            && let Ok(identity) =
                nbfc_config::identity_report(selected, self.view.sensor_config.as_ref())
        {
            let identity = identity.with_live_evidence(
                &self.view.snapshot,
                status,
                crate::model::now_ms(),
                3_000,
            );
            lines.push(Line::from(vec![
                Span::styled("Config         ", theme::muted()),
                Span::raw(format!(
                    "{}  ·  {} logical channel(s)",
                    identity.selected_config,
                    identity.fans.len()
                )),
            ]));
            for fan in &identity.fans {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {:<12}", fan.name), theme::muted()),
                    Span::raw(format!(
                        "{} · {} · sensors {} · readback {}",
                        fan.role.to_string().to_ascii_lowercase(),
                        fan.register_pair,
                        if fan.sensor_sources.is_empty() {
                            "none".to_string()
                        } else {
                            fan.sensor_sources.join(",")
                        },
                        fan.readback_evidence.interpretation.label()
                    )),
                ]));
            }
        }

        push_section_header(&mut lines, "Profiles");
        if let Some(custom) = self
            .supervisor
            .status
            .as_ref()
            .and_then(|status| status.get("custom"))
        {
            lines.push(Line::from(vec![
                Span::styled("Custom store   ", theme::muted()),
                Span::raw(format!(
                    "{} profile(s)  ·  {}",
                    custom
                        .get("profiles")
                        .and_then(Value::as_array)
                        .map(Vec::len)
                        .unwrap_or(0),
                    custom
                        .get("path")
                        .and_then(Value::as_str)
                        .unwrap_or("path unavailable")
                )),
            ]));
            if let Some(error) = custom.get("error").and_then(Value::as_str) {
                lines.push(Line::from(Span::styled(
                    format!("Store error: {error}"),
                    Style::default().fg(theme::ERROR),
                )));
            }
            for warning in custom
                .get("warnings")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                lines.push(Line::from(Span::styled(
                    format!("Store warning: {warning}"),
                    Style::default().fg(theme::WARN),
                )));
            }
        } else {
            lines.push(Line::from(Span::styled(
                "Custom store unavailable",
                theme::muted(),
            )));
        }

        push_section_header(&mut lines, "Warnings");
        let warnings = warnings_from_view(&self.view, Some(&self.supervisor));
        let reasons = self.safety_reasons();
        if !reasons.is_empty() {
            lines.push(Line::from(Span::styled(
                truncate_for_width(&reasons, area.width.saturating_sub(4) as usize),
                Style::default().fg(theme::ERROR),
            )));
        }
        if warnings.is_empty() {
            lines.push(Line::from(Span::styled(
                "✓ no active warnings",
                Style::default().fg(theme::ACTIVE),
            )));
        }
        for warning in &warnings {
            let color = match warning.severity {
                Severity::Critical => theme::ERROR,
                Severity::Warning => theme::WARN,
                Severity::Info => theme::ACCENT,
                Severity::Success => theme::ACTIVE,
            };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:<9}", warning.severity.label()),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(warning.title.clone(), Style::default().fg(color)),
            ]));
            lines.push(Line::from(Span::styled(
                format!("          {} — next: {}", warning.detail, warning.action),
                theme::muted(),
            )));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .block(theme::panel("System"))
                .wrap(Wrap { trim: true })
                .scroll((self.scroll, 0)),
            area,
        );
    }

    fn render_diagnostics(&self, frame: &mut ratatui::Frame, area: Rect) {
        let Some(report) = &self.diagnostics else {
            frame.render_widget(
                Paragraph::new("Diagnostics not loaded. Press r to refresh.")
                    .block(theme::panel("Diagnostics")),
                area,
            );
            return;
        };
        let mut lines = Vec::new();
        push_section_header(&mut lines, "Environment");
        lines.push(Line::from(Span::styled(
            format!(
                "{} · kernel {} · {} {}",
                report.environment.os.as_deref().unwrap_or("unknown OS"),
                report.environment.kernel.as_deref().unwrap_or("?"),
                report.environment.vendor.as_deref().unwrap_or(""),
                report.environment.model.as_deref().unwrap_or("")
            ),
            theme::muted(),
        )));
        if let Some(error) = &report.sensor_source_error {
            lines.push(Line::from(Span::styled(
                format!("Sensor source: {error}"),
                Style::default().fg(theme::ERROR),
            )));
        }

        push_section_header(&mut lines, "Summary");
        let summary = &report.summary;
        lines.push(Line::from(vec![
            Span::styled("Operational     ", theme::muted()),
            Span::styled(
                if summary.operational { "yes" } else { "no" },
                Style::default()
                    .fg(if summary.operational {
                        theme::ACTIVE
                    } else {
                        theme::ERROR
                    })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("   Supervisor ", theme::muted()),
            Span::raw(summary.supervisor.label().to_string()),
            Span::styled("   Manual path ", theme::muted()),
            Span::styled(
                if summary.fan_control_available {
                    "available"
                } else {
                    "blocked"
                },
                Style::default().fg(if summary.fan_control_available {
                    theme::ACTIVE
                } else {
                    theme::WARN
                }),
            ),
            Span::styled("   Thermal ", theme::muted()),
            Span::styled(
                if summary.thermal_reliable {
                    "reliable"
                } else {
                    "degraded"
                },
                Style::default().fg(if summary.thermal_reliable {
                    theme::ACTIVE
                } else {
                    theme::WARN
                }),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Issues          ", theme::muted()),
            Span::styled(
                format!("{} critical", summary.critical_issues),
                Style::default().fg(if summary.critical_issues > 0 {
                    theme::ERROR
                } else {
                    theme::ACTIVE
                }),
            ),
            Span::styled("  ·  ", theme::muted()),
            Span::styled(
                format!("{} warnings", summary.warning_issues),
                Style::default().fg(if summary.warning_issues > 0 {
                    theme::WARN
                } else {
                    theme::ACTIVE
                }),
            ),
            Span::styled(
                format!("  ·  overall {:?}", summary.overall),
                theme::muted(),
            ),
        ]));

        push_section_header(&mut lines, "Control");
        lines.push(Line::from(vec![
            Span::styled("Stage           ", theme::muted()),
            Span::raw(report.control.stage.label().to_string()),
            Span::styled("   Mode ", theme::muted()),
            Span::raw(report.control.mode.label().to_string()),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Eligible fans   ", theme::muted()),
            Span::raw(format!("{:?}", report.control.eligible_fans)),
            Span::styled("   Blocked ", theme::muted()),
            Span::styled(
                format!("{:?}", report.control.blocked_fans),
                if report.control.blocked_fans.is_empty() {
                    theme::muted()
                } else {
                    Style::default().fg(theme::WARN)
                },
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Calibration     ", theme::muted()),
            Span::styled(
                report.calibration_readiness.state.label().to_string(),
                Style::default().fg(theme::WARN),
            ),
            Span::styled("  ·  test authorized false", theme::muted()),
        ]));
        if let Some(missing) = report.calibration_readiness.missing_evidence.first() {
            lines.push(Line::from(Span::styled(
                format!("Missing evidence: {missing}"),
                Style::default().fg(theme::WARN),
            )));
        }
        for fan in &report.control.fans {
            if !fan.policy_eligible {
                lines.push(Line::from(Span::styled(
                    format!(
                        "Fan #{} {} blocked: {}",
                        fan.index,
                        fan.name,
                        fan.reasons.join("; ")
                    ),
                    Style::default().fg(theme::WARN),
                )));
            }
        }
        for blocker in &report.control.blockers {
            lines.push(Line::from(Span::styled(
                format!("Blocker: {blocker}"),
                Style::default().fg(theme::WARN),
            )));
        }

        let criticals: Vec<_> = report
            .checks
            .iter()
            .filter(|check| check.status == CheckStatus::Fail)
            .collect();
        if !criticals.is_empty() {
            push_section_header(&mut lines, "Critical issues");
            for check in criticals {
                push_check(&mut lines, check);
            }
        }
        for group in [
            CheckGroup::Backend,
            CheckGroup::Hardware,
            CheckGroup::Thermal,
            CheckGroup::Fans,
            CheckGroup::Control,
            CheckGroup::Security,
        ] {
            let checks: Vec<_> = report
                .checks_in(group)
                .filter(|check| check.status != CheckStatus::Fail)
                .collect();
            if checks.is_empty() {
                continue;
            }
            push_section_header(&mut lines, group.label());
            for check in checks {
                push_check(&mut lines, check);
            }
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Use `neosense diagnose --json` for the complete machine-readable report.",
            theme::muted(),
        )));
        frame.render_widget(
            Paragraph::new(lines)
                .block(theme::panel("Diagnostics"))
                .wrap(Wrap { trim: true })
                .scroll((self.scroll, 0)),
            area,
        );
    }

    fn render_footer(&self, frame: &mut ratatui::Frame, area: Rect) {
        let (text, style) = if self.editing_fan.is_some() || self.editing_both.is_some() {
            (
                "Type a target between 20 and 100 · Enter to review · Esc to cancel".to_string(),
                Style::default().fg(theme::WARN),
            )
        } else if self.pending_both_set.is_some() || self.pending_fan_set.is_some() {
            (
                format!(
                    "{}  (y / n)",
                    self.message
                        .as_deref()
                        .unwrap_or("Apply the manual target?")
                ),
                Style::default().fg(theme::WARN),
            )
        } else if let Some(message) = &self.message {
            (
                truncate_for_width(message, area.width.saturating_sub(4) as usize),
                Style::default().fg(theme::ACCENT),
            )
        } else {
            (self.footer_hint(area.width), theme::muted())
        };
        let mut line = text;
        if self.message.is_none()
            && self.pending_fan_set.is_none()
            && self.pending_both_set.is_none()
            && !self.supervisor.state.is_connected()
        {
            line.push_str("  ·  read-only");
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(theme::muted())
            .padding(Padding::horizontal(1));
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(line, style))).block(block),
            area,
        );
    }

    fn footer_hint(&self, width: u16) -> String {
        let narrow = width < 90;
        match self.tab {
            Tab::Dashboard => {
                if narrow {
                    "s Silent · b Balanced · m Maximum · a Auto · ? help".to_string()
                } else {
                    "s Silent  ·  b Balanced  ·  m Maximum  ·  a Auto  ·  2 Fan Control  ·  ? help"
                        .to_string()
                }
            }
            Tab::Fans => {
                if narrow {
                    "↑/↓ select · m set · b both · +/− · a auto · A all · ?".to_string()
                } else {
                    "↑/↓ select  ·  m set target  ·  b both  ·  +/− adjust  ·  a automatic  ·  A all  ·  ? help"
                        .to_string()
                }
            }
            Tab::Presets => {
                if narrow {
                    "↑/↓ select · Enter apply · a Auto · ?".to_string()
                } else {
                    "↑/↓ select  ·  Enter applies immediately  ·  a Automatic  ·  ? help"
                        .to_string()
                }
            }
            Tab::Profiles => {
                if narrow {
                    "↑/↓ select · Enter apply · p preview · d stop · e edit".to_string()
                } else {
                    "↑/↓ select  ·  Enter apply  ·  p preview  ·  d deactivate  ·  e edit  ·  n new  ·  ? help"
                        .to_string()
                }
            }
            Tab::Sensors => {
                if narrow {
                    "q quit · Tab views · r refresh · j/k scroll · ?".to_string()
                } else {
                    "q quit  ·  Tab/←→ views  ·  r refresh  ·  j/k or PgUp/PgDn scroll  ·  ? help"
                        .to_string()
                }
            }
            Tab::System | Tab::Diagnostics => {
                if narrow {
                    "q quit · Tab views · r refresh · j/k scroll · ?".to_string()
                } else {
                    "q quit  ·  Tab/←→ views  ·  r refresh  ·  j/k or PgUp/PgDn scroll  ·  ? help"
                        .to_string()
                }
            }
        }
    }

    fn render_help(&self, frame: &mut ratatui::Frame, area: Rect) {
        let popup = centered_rect(78, 76, area);
        frame.render_widget(Clear, popup);
        let heading = |text: &'static str| {
            Line::from(Span::styled(
                text,
                Style::default()
                    .fg(theme::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ))
        };
        let row = |keys: &'static str, text: &'static str| {
            Line::from(vec![
                Span::styled(format!("  {keys:<14}"), theme::selected()),
                Span::raw(text),
            ])
        };
        let lines = vec![
            Line::from(vec![
                Span::styled("NeoSense", theme::heading()),
                Span::styled(" — help", theme::muted()),
            ]),
            Line::from(Span::styled(TAGLINE, theme::muted())),
            Line::from(""),
            heading("Navigation"),
            row("q / Esc", "quit"),
            row("Tab / ← →", "change view"),
            row("1 · 2 · 3", "Dashboard · Fan Control · Presets"),
            row("4 · 5 · 6 · 7", "Profiles · Sensors · System · Diagnostics"),
            row("r", "refresh now"),
            Line::from(""),
            heading("Cooling"),
            row("s / b / m", "Dashboard: Silent / Balanced / Maximum"),
            row("a", "Automatic (NBFC) mode"),
            row("A", "all fans automatic (any view)"),
            row("2 then m", "Fan Control: set the selected target"),
            row("2 then b", "Fan Control: set both fans to one target"),
            row("2 then a", "Fan Control: selected fan automatic"),
            row(
                "3 then Enter",
                "Presets: apply the selected mode immediately",
            ),
            row("4 then Enter", "Profiles: apply the selected profile"),
            row("4 then p", "Profiles: preview without writing"),
            Line::from(""),
            heading("Good to know"),
            Line::from("  Mode changes apply immediately. Manual percentage changes ask for y/n."),
            Line::from("  EC readback is not measured RPM; blocked fans always stay automatic."),
            Line::from(""),
            Line::from(Span::styled("  ? / Esc close help", theme::muted())),
        ];
        let visible = popup.height.saturating_sub(2) as usize;
        let max_scroll = lines.len().saturating_sub(visible) as u16;
        let scroll = self.help_scroll.min(max_scroll);
        frame.render_widget(
            Paragraph::new(lines)
                .block(theme::panel_focused("Help — j/k scroll · ? close"))
                .wrap(Wrap { trim: true })
                .scroll((scroll, 0)),
            popup,
        );
    }
}

fn truncate_for_width(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    if width <= 3 {
        return ".".repeat(width);
    }
    let mut output: String = text.chars().take(width - 3).collect();
    output.push_str("...");
    output
}

fn header_height(area: Rect, ascii_logo: bool) -> u16 {
    if area.width >= 90 && area.height >= 30 {
        if area.width >= 118 { 7 } else { 8 }
    } else if !ascii_logo && area.width >= 70 && area.height >= 20 {
        3
    } else {
        2
    }
}

fn unicode_logo_supported() -> bool {
    if let Ok(value) = std::env::var("NEOSENSE_ASCII") {
        let value = value.trim().to_ascii_lowercase();
        if matches!(value.as_str(), "1" | "true" | "yes" | "on") {
            return false;
        }
    }
    for key in ["LC_ALL", "LC_CTYPE", "LANG"] {
        if let Ok(value) = std::env::var(key) {
            if value.is_empty() {
                continue;
            }
            let value = value.to_ascii_lowercase();
            return value.contains("utf-8") || value.contains("utf8");
        }
    }
    true
}

fn profile_display_name(name: &str) -> String {
    name.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn preset_index_from_safety(safety: &Value) -> Option<usize> {
    let leases = safety.get("preset")?.as_array()?;
    if leases.is_empty() {
        return None;
    }
    let percents: Vec<f64> = leases
        .iter()
        .map(|lease| lease.get("percent").and_then(Value::as_f64))
        .collect::<Option<Vec<_>>>()?;
    let first = *percents.first()?;
    if percents.iter().any(|percent| (percent - first).abs() > 0.5) {
        return None;
    }
    preset_index_for_percent(first)
}

fn preset_index_for_percent(percent: f64) -> Option<usize> {
    presets::builtins().iter().position(|preset| {
        (preset.cpu_percent - percent).abs() <= 0.5 && (preset.gpu_percent - percent).abs() <= 0.5
    })
}

/// Solid tag used for mode and state labels. Uses a filled background so the
/// state is legible without relying on colour alone.
fn mode_tag(label: &str, color: Color) -> Span<'static> {
    Span::styled(
        format!(" {label} "),
        Style::default()
            .fg(Color::Black)
            .bg(color)
            .add_modifier(Modifier::BOLD),
    )
}

fn percent_bar(percent: f64, width: usize) -> Vec<Span<'static>> {
    let clamped = percent.clamp(0.0, 100.0);
    let filled = ((clamped / 100.0) * width as f64).round() as usize;
    vec![
        Span::styled("█".repeat(filled), Style::default().fg(theme::ACCENT)),
        Span::styled(
            "░".repeat(width.saturating_sub(filled)),
            Style::default().fg(theme::MUTED),
        ),
    ]
}

fn trust_color(trust: crate::model::Trust) -> Color {
    use crate::model::Trust;
    match trust {
        Trust::Verified => theme::ACTIVE,
        Trust::Trusted => theme::ACCENT,
        Trust::Provisional => theme::WARN,
        Trust::Untrusted => theme::MUTED,
    }
}

fn supervisor_short(state: SupervisorState) -> &'static str {
    match state {
        SupervisorState::Healthy => "healthy",
        SupervisorState::Connected => "connected",
        SupervisorState::Degraded => "degraded",
        SupervisorState::NotRunning => "offline",
        SupervisorState::PermissionDenied => "no access",
        SupervisorState::StaleSocket => "stale",
        SupervisorState::ProtocolError => "protocol",
    }
}

fn supervisor_style(state: SupervisorState) -> (Color, &'static str) {
    match state {
        SupervisorState::Healthy => (Color::Green, "healthy"),
        SupervisorState::Connected => (Color::Cyan, "connected"),
        SupervisorState::Degraded => (Color::Yellow, "degraded"),
        SupervisorState::NotRunning => (Color::Red, "not running"),
        SupervisorState::PermissionDenied => (Color::Red, "permission denied"),
        SupervisorState::StaleSocket => (Color::Red, "stale socket"),
        SupervisorState::ProtocolError => (Color::Red, "protocol error"),
    }
}

fn push_section_header(lines: &mut Vec<Line<'static>>, title: &str) {
    if !lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled(
        title.to_string(),
        Style::default()
            .fg(theme::ACCENT)
            .add_modifier(Modifier::BOLD),
    )));
}

fn push_check(lines: &mut Vec<Line<'static>>, check: &crate::diagnostics::DiagnosticCheck) {
    let (label, color) = match check.status {
        CheckStatus::Ok => ("OK  ", Color::Green),
        CheckStatus::Info => ("INFO", Color::Cyan),
        CheckStatus::Warning => ("WARN", Color::Yellow),
        CheckStatus::Fail => ("CRIT", Color::Red),
    };
    lines.push(Line::from(vec![
        Span::styled(format!("[{label}] "), Style::default().fg(color)),
        Span::raw(format!("{:<26} {}", check.title, check.message)),
    ]));
}

fn opt_percent(value: Option<f64>) -> String {
    value
        .filter(|v| *v >= 0.0)
        .map(|v| format!("{v:.1}%"))
        .unwrap_or_else(|| "n/a".into())
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let [middle] = Layout::vertical([Constraint::Percentage(percent_y)])
        .flex(ratatui::layout::Flex::Center)
        .areas(r);
    let [area] = Layout::horizontal([Constraint::Percentage(percent_x)])
        .flex(ratatui::layout::Flex::Center)
        .areas(middle);
    area
}

pub fn run(socket: Option<PathBuf>) -> Result<(), String> {
    if !stdout().is_terminal() {
        return Err("the TUI requires an interactive terminal".to_string());
    }
    enable_raw_mode().map_err(|e| format!("raw mode: {e}"))?;
    let mut out = stdout();
    execute!(out, EnterAlternateScreen).map_err(|e| format!("alternate screen: {e}"))?;
    let backend = CrosstermBackend::new(out);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(error) => {
            let _ = disable_raw_mode();
            let _ = execute!(stdout(), LeaveAlternateScreen);
            return Err(format!("terminal: {error}"));
        }
    };
    let mut app = TuiApp::new(socket);

    let result = run_loop(&mut terminal, &mut app);
    let _ = disable_raw_mode();
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .map_err(|e| format!("leave alternate screen: {e}"))?;
    let _ = terminal.show_cursor();
    result
}

fn run_loop<B: Backend>(terminal: &mut Terminal<B>, app: &mut TuiApp) -> Result<(), String> {
    loop {
        terminal
            .draw(|frame| app.render(frame))
            .map_err(|e| format!("TUI draw: {e}"))?;
        app.maybe_refresh();
        if app.should_quit {
            return Ok(());
        }
        if event::poll(Duration::from_millis(100)).map_err(|e| format!("TUI event: {e}"))? {
            match event::read().map_err(|e| format!("event read: {e}"))? {
                Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key.code),
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
        if let Some(request) = app.pending_editor.take() {
            let editor = std::env::var("VISUAL")
                .or_else(|_| std::env::var("EDITOR"))
                .unwrap_or_else(|_| "vi".to_string());
            let _ = disable_raw_mode();
            let _ = execute!(stdout(), LeaveAlternateScreen);
            let _ = terminal.show_cursor();
            let status = std::process::Command::new(&editor)
                .arg(&request.path)
                .status();
            let _ = enable_raw_mode();
            let _ = execute!(stdout(), EnterAlternateScreen);
            let _ = terminal.clear();
            match status {
                Ok(status) if status.success() => app.finish_editor(request),
                Ok(_) => {
                    let _ = std::fs::remove_file(&request.path);
                    app.message = Some(format!("Editor '{editor}' exited unsuccessfully"));
                }
                Err(error) => {
                    let _ = std::fs::remove_file(&request.path);
                    app.message = Some(format!("Cannot run editor '{editor}': {error}"));
                }
            }
        }
    }
}

fn fan_index_list(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_array)
        .map(|items| {
            format!(
                "[{}]",
                items
                    .iter()
                    .filter_map(Value::as_u64)
                    .map(|index| index.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .unwrap_or_else(|| "n/a".to_string())
}

// The helper is kept here to make the TUI render tests independent from the
// real terminal and hardware.
#[cfg(test)]
fn buffer_text(terminal: &Terminal<ratatui::backend::TestBackend>) -> String {
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(ratatui::buffer::Cell::symbol)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SensorSnapshot;
    use crate::nbfc::{NbfcFan, NbfcStatus};
    use crate::service::ServiceStatus;

    fn empty_view() -> RuntimeView {
        RuntimeView {
            snapshot: SensorSnapshot::empty(),
            sensor_source_error: None,
            version: None,
            capabilities: None,
            nbfc_runtime: None,
            last_validation: None,
            status: Some(NbfcStatus {
                version: None,
                read_only: Some(false),
                selected_config: Some("test".into()),
                fans: vec![NbfcFan {
                    index: 0,
                    name: "CPU fan".into(),
                    temperature: Some(50.0),
                    critical_temperature: Some(95.0),
                    auto_mode: Some(true),
                    critical: Some(false),
                    current_speed: Some(0.0),
                    target_speed: Some(0.0),
                    requested_speed: Some(-1.0),
                    speed_steps: Some(100),
                }],
                warnings: Vec::new(),
            }),
            service: ServiceStatus::default(),
            sensor_config: None,
            backend_error: None,
            supervisor: crate::supervisor::probe(std::path::Path::new(
                "/nonexistent-neosense-test.sock",
            )),
        }
    }

    fn view_with_fans() -> RuntimeView {
        let version = crate::nbfc::NbfcVersion::parse("nbfc 0.5.4").unwrap();
        let mut capabilities = crate::nbfc::NbfcCapabilities::from_version(version);
        capabilities.access_controlled = true;
        let cpu = crate::model::SensorSample::new(
            "test:cpu".into(),
            crate::model::SensorRole::CpuPackage,
            crate::model::SensorKind::Temperature,
            Some(55.0),
            crate::model::Unit::Celsius,
            Some("55000".into()),
            "millidegree_c",
            crate::model::SensorProvider::Hwmon,
            "coretemp".into(),
            Some("/sys/test/cpu".into()),
            Some("Package id 0".into()),
            None,
        )
        .with_trust(crate::model::Trust::Verified);
        RuntimeView {
            snapshot: SensorSnapshot {
                sensors: vec![cpu],
                errors: Vec::new(),
                collected_at_ms: crate::model::now_ms(),
            },
            sensor_source_error: None,
            version: Some(version),
            capabilities: Some(capabilities),
            nbfc_runtime: None,
            last_validation: None,
            status: Some(NbfcStatus {
                version: None,
                read_only: Some(false),
                selected_config: Some("test".into()),
                fans: vec![NbfcFan {
                    index: 0,
                    name: "CPU fan".into(),
                    temperature: Some(55.0),
                    critical_temperature: Some(95.0),
                    auto_mode: Some(true),
                    critical: Some(false),
                    current_speed: Some(35.0),
                    target_speed: Some(35.0),
                    requested_speed: Some(0.0),
                    speed_steps: Some(100),
                }],
                warnings: Vec::new(),
            }),
            service: ServiceStatus::default(),
            sensor_config: Some(crate::nbfc::NbfcSensorConfig {
                fans: vec![crate::nbfc::NbfcFanSensor {
                    index: 0,
                    name: "CPU fan".into(),
                    sensors: vec!["@CPU".into()],
                    algorithm: Some("Average".into()),
                }],
            }),
            backend_error: None,
            supervisor: crate::supervisor::SupervisorProbe {
                state: crate::supervisor::SupervisorState::Healthy,
                socket: "/run/neosense/neosensed.sock".into(),
                socket_exists: true,
                socket_mode: Some("660".into()),
                socket_uid: Some(0),
                socket_gid: Some(0),
                health: Some(true),
                reason: None,
                error: None,
                status: None,
            },
        }
    }

    fn render_once(app: &mut TuiApp) -> String {
        render_size(app, 100, 32)
    }

    fn render_size(app: &mut TuiApp, width: u16, height: u16) -> String {
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        buffer_text(&terminal)
    }

    fn serve_once<F>(
        socket: std::path::PathBuf,
        expected_method: &'static str,
        verify: F,
        response: Value,
    ) -> std::thread::JoinHandle<()>
    where
        F: Fn(&crate::ipc::Request) + Send + 'static,
    {
        use crate::ipc::{Response, read_message, write_message};
        use std::io::BufReader;
        use std::os::unix::net::UnixListener;

        let listener = UnixListener::bind(&socket).unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            drop(listener);
            let mut reader = BufReader::new(&stream);
            let request: crate::ipc::Request = read_message(&mut reader).unwrap();
            assert_eq!(request.method, expected_method);
            verify(&request);
            let mut stream = stream;
            write_message(&mut stream, &Response::ok(request.id, response)).unwrap();
        })
    }

    fn view_with_two_fans() -> RuntimeView {
        let mut view = view_with_fans();
        view.status.as_mut().unwrap().fans.push(NbfcFan {
            index: 1,
            name: "GPU fan".into(),
            temperature: Some(50.0),
            critical_temperature: Some(95.0),
            auto_mode: Some(true),
            critical: Some(false),
            current_speed: Some(40.0),
            target_speed: Some(40.0),
            requested_speed: Some(-1.0),
            speed_steps: Some(100),
        });
        view.sensor_config
            .as_mut()
            .unwrap()
            .fans
            .push(crate::nbfc::NbfcFanSensor {
                index: 1,
                name: "GPU fan".into(),
                sensors: vec!["@CPU".into()],
                algorithm: Some("Average".into()),
            });
        view
    }

    #[test]
    fn dashboard_and_tabs_render() {
        let mut app = TuiApp::from_view(empty_view());
        let text = render_once(&mut app);
        assert!(text.contains(TAGLINE) || text.contains("NeoSense"));
        assert!(text.contains("Dashboard"));
        for tab in Tab::ALL {
            app.tab = tab;
            let text = render_once(&mut app);
            assert!(!text.is_empty());
        }
        // Manual-path eligibility must not be labelled as fan control.
        app.select_tab(Tab::Diagnostics);
        let diagnostics = render_once(&mut app);
        assert!(diagnostics.contains("Manual path"), "{diagnostics}");
        assert!(!diagnostics.contains("Fan control"), "{diagnostics}");
    }

    #[test]
    fn tab_navigation_moves_between_views_and_auto_is_direct() {
        let mut app = TuiApp::from_view(empty_view());
        app.handle_key(KeyCode::Tab);
        assert_eq!(app.tab, Tab::Fans);
        app.handle_key(KeyCode::BackTab);
        assert_eq!(app.tab, Tab::Dashboard);
        app.handle_key(KeyCode::Char('a'));
        assert!(
            app.message.as_deref().unwrap_or("").contains("read-only"),
            "{:?}",
            app.message
        );
        app.show_help = true;
        app.handle_key(KeyCode::Esc);
        assert!(!app.show_help);
    }

    #[test]
    fn dashboard_shortcuts_apply_modes_without_confirmation() {
        let mut app = TuiApp::from_view(view_with_fans());
        app.supervisor.state = SupervisorState::Healthy;
        app.socket = None;
        for (key, index, title) in [
            ('s', 0, "Silent"),
            ('b', 1, "Balanced"),
            ('m', 2, "Maximum"),
        ] {
            app.handle_key(KeyCode::Char(key));
            assert_eq!(app.preset_index, index);
            assert!(
                app.message
                    .as_deref()
                    .unwrap_or("")
                    .contains("No supervisor socket"),
                "{title}: {:?}",
                app.message
            );
            app.message = None;
        }
    }

    #[test]
    fn small_and_large_terminal_sizes_render() {
        let mut app = TuiApp::from_view(empty_view());
        for (width, height) in [(80, 24), (40, 20), (160, 48)] {
            for tab in Tab::ALL {
                app.tab = tab;
                let text = render_size(&mut app, width, height);
                assert!(!text.is_empty(), "empty render at {width}x{height}");
            }
        }
    }

    #[test]
    fn dashboard_prioritises_mode_fans_and_shortcuts() {
        let mut app = TuiApp::from_view(view_with_two_fans());
        app.tab = Tab::Dashboard;
        let text = render_size(&mut app, 120, 36);
        assert!(text.contains("Cooling mode"), "{text}");
        assert!(text.contains("Fans"), "{text}");
        assert!(text.contains("Silent"), "{text}");
        assert!(text.contains("Maximum"), "{text}");
        assert!(text.contains("CPU fan"), "{text}");
        assert!(text.contains("GPU fan"), "{text}");
        assert!(text.contains("Auto"), "{text}");
    }

    #[test]
    fn diagnostics_view_shows_control_stage_and_blockers() {
        let mut app = TuiApp::from_view(empty_view());
        app.diagnostics = Some(diagnose(&app.view));
        app.tab = Tab::Diagnostics;
        let text = render_size(&mut app, 120, 40);
        assert!(text.contains("Control"), "{text}");
        assert!(text.contains("Supervisor IPC"), "{text}");
    }

    #[test]
    fn very_narrow_terminals_render_every_view_without_panic() {
        let mut app = TuiApp::from_view(empty_view());
        for (width, height) in [(30, 12), (20, 10), (24, 8)] {
            for tab in Tab::ALL {
                app.tab = tab;
                let text = render_size(&mut app, width, height);
                assert!(!text.is_empty(), "empty render at {width}x{height}");
            }
        }
    }

    #[test]
    fn fan_control_tab_edits_target_and_confirms_manual_writes() {
        let mut app = TuiApp::from_view(view_with_fans());
        app.supervisor.state = SupervisorState::Healthy;
        app.tab = Tab::Fans;
        app.handle_key(KeyCode::Char('m'));
        assert!(app.editing_fan.is_some());
        app.handle_key(KeyCode::Char('6'));
        app.handle_key(KeyCode::Char('0'));
        app.handle_key(KeyCode::Enter);
        assert_eq!(app.pending_fan_set, Some((0, 60.0)));
        app.handle_key(KeyCode::Char('n'));
        assert!(app.pending_fan_set.is_none());

        app.handle_key(KeyCode::Char('+'));
        assert!(app.pending_fan_set.is_some());
        app.handle_key(KeyCode::Char('n'));

        app.handle_key(KeyCode::Char('a'));
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("No supervisor socket")
        );
        app.message = None;
        app.handle_key(KeyCode::Char('A'));
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("No supervisor socket")
        );
    }

    #[test]
    fn fan_control_view_uses_discovered_inventory() {
        let mut app = TuiApp::from_view(view_with_fans());
        app.tab = Tab::Fans;
        let text = render_size(&mut app, 110, 32);
        assert!(text.contains("Fans"), "{text}");
        assert!(text.contains("CPU fan"), "{text}");
        assert!(text.contains("Target"), "{text}");
        assert!(text.contains("verified fresh"), "{text}");
        assert!(text.contains("m set target"), "{text}");
    }

    #[test]
    fn profile_apply_and_deactivate_are_immediate() {
        let mut app = TuiApp::from_view(view_with_fans());
        app.supervisor.state = SupervisorState::Healthy;
        app.socket = None;
        app.tab = Tab::Profiles;
        app.profile_index = 1;
        app.handle_key(KeyCode::Enter);
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("No supervisor socket"),
            "{:?}",
            app.message
        );
        app.handle_key(KeyCode::Char('d'));
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("No supervisor socket"),
            "{:?}",
            app.message
        );
    }

    #[test]
    fn profile_view_reports_partial_eligibility() {
        let mut view = view_with_fans();
        view.service.active_state = Some("active".into());
        view.service.sub_state = Some("running".into());
        view.supervisor.status = Some(serde_json::json!({
            "profile": {"mode": null},
            "safety": {"state": "safe_auto"}
        }));
        view.capabilities.as_mut().unwrap().socket = Some(crate::nbfc::NbfcSocketSecurity {
            path: "/run/nbfc_service.socket".into(),
            exists: true,
            is_socket: true,
            mode: Some("600".into()),
            uid: Some(0),
            gid: Some(0),
            group_name: Some("root".into()),
            ordinary_group_members: 0,
            group_or_world_writable: false,
            acl_present: false,
            supervisor_uid: 0,
            supervisor_gid: 0,
            supervisor_groups_verified: true,
            identity_source: "test".into(),
            accessible_to_supervisor: true,
            access_controlled: true,
            error: None,
        });
        view.status.as_mut().unwrap().fans.push(NbfcFan {
            index: 1,
            name: "GPU fan".into(),
            temperature: Some(55.0),
            critical_temperature: Some(95.0),
            auto_mode: Some(true),
            critical: Some(false),
            current_speed: Some(25.0),
            target_speed: Some(25.0),
            requested_speed: Some(0.0),
            speed_steps: Some(100),
        });
        view.sensor_config
            .as_mut()
            .unwrap()
            .fans
            .push(crate::nbfc::NbfcFanSensor {
                index: 1,
                name: "GPU fan".into(),
                sensors: vec!["@CPU".into()],
                algorithm: Some("Average".into()),
            });
        let mut app = TuiApp::from_view(view);
        app.tab = Tab::Profiles;
        app.profile_index = 1;
        let text = render_size(&mut app, 120, 34);
        assert!(text.contains("partial"), "{text}");
        assert!(text.contains("blocked [1]"), "{text}");
    }

    #[test]
    fn profile_view_marks_a_failed_profile_inactive() {
        let mut app = TuiApp::from_view(view_with_fans());
        app.supervisor.status = Some(serde_json::json!({
            "profile": {
                "selected": "quiet",
                "active": false,
                "reason": "profile stopped: fan 0 target not confirmed; automatic mode confirmed by backend"
            },
            "safety": {"state": "failsafe", "reasons": ["profile stopped"]}
        }));
        app.tab = Tab::Profiles;
        let text = render_size(&mut app, 120, 32);
        assert!(text.contains("Quiet inactive"), "{text}");
    }

    #[test]
    fn profile_view_shows_selected_curve_points() {
        let mut app = TuiApp::from_view(view_with_fans());
        app.tab = Tab::Profiles;
        app.profile_index = 1;
        let text = render_size(&mut app, 120, 34);
        assert!(text.contains("Selected · quiet"), "{text}");
        assert!(text.contains("40°C"), "{text}");
        assert!(text.contains("20%"), "{text}");
        assert!(text.contains("Eligible fans"), "{text}");
    }

    #[test]
    fn profile_view_advertises_apply_and_deactivate() {
        let mut app = TuiApp::from_view(view_with_fans());
        app.tab = Tab::Profiles;
        let text = render_size(&mut app, 120, 32);
        assert!(text.contains("Enter apply"), "{text}");
        assert!(text.contains("d deactivate"), "{text}");
    }

    #[test]
    fn fan_control_both_edit_requests_combined_confirmation() {
        let mut app = TuiApp::from_view(view_with_fans());
        app.tab = Tab::Fans;
        app.handle_key(KeyCode::Char('b'));
        assert!(app.editing_both.is_some());
        app.handle_key(KeyCode::Char('6'));
        app.handle_key(KeyCode::Char('0'));
        app.handle_key(KeyCode::Enter);
        assert_eq!(app.pending_both_set, Some(60.0));
        assert!(app.message.as_deref().unwrap_or("").contains("CPU and GPU"));
        app.handle_key(KeyCode::Char('n'));
        assert!(app.pending_both_set.is_none());
    }

    #[test]
    fn invalid_or_cancelled_manual_input_never_requests_a_write() {
        let mut app = TuiApp::from_view(view_with_fans());
        app.tab = Tab::Fans;
        app.handle_key(KeyCode::Char('m'));
        app.handle_key(KeyCode::Enter);
        assert!(app.editing_fan.is_some());
        assert!(app.pending_fan_set.is_none());
        app.handle_key(KeyCode::Char('0'));
        app.handle_key(KeyCode::Enter);
        assert!(app.editing_fan.is_some());
        assert!(app.pending_fan_set.is_none());
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("0% is never allowed")
        );
        app.handle_key(KeyCode::Backspace);
        app.handle_key(KeyCode::Char('1'));
        app.handle_key(KeyCode::Char('0'));
        app.handle_key(KeyCode::Char('1'));
        app.handle_key(KeyCode::Enter);
        assert!(app.editing_fan.is_some());
        assert!(app.pending_fan_set.is_none());
        app.handle_key(KeyCode::Esc);
        assert!(app.editing_fan.is_none());
        assert!(app.pending_fan_set.is_none());
    }

    #[test]
    fn enter_on_safe_auto_restores_automatic_mode() {
        let mut app = TuiApp::from_view(empty_view());
        app.tab = Tab::Profiles;
        app.profile_index = 0;
        app.handle_key(KeyCode::Enter);
        assert!(
            app.message.as_deref().unwrap_or("").contains("read-only"),
            "{:?}",
            app.message
        );
    }

    #[test]
    fn profile_apply_sends_exactly_one_supervisor_request() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("tui-apply.sock");
        let server = serve_once(
            socket.clone(),
            "profile.apply",
            |request| {
                assert_eq!(request.params["name"], "quiet");
                assert_eq!(request.params["confirm"], true);
            },
            serde_json::json!({
                "hub": false,
                "hardware_applied": true,
                "partial": true,
                "applied_fans": [0],
                "blocked_fans": [1]
            }),
        );
        let mut app = TuiApp::from_view(view_with_fans());
        app.socket = Some(socket);
        app.supervisor.state = SupervisorState::Healthy;
        app.tab = Tab::Profiles;
        app.profile_index = 1;
        app.handle_key(KeyCode::Enter);
        server.join().unwrap();
        let message = app.message.as_deref().unwrap_or("");
        assert!(message.contains("active"), "{message}");
        assert!(message.contains("partial"), "{message}");
        assert!(message.contains("[0]"), "{message}");
        assert!(message.contains("[1]"), "{message}");
    }

    #[test]
    fn profile_preview_sends_select_and_no_apply() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("tui-select.sock");
        let server = serve_once(
            socket.clone(),
            "profile.select",
            |_| {},
            serde_json::json!({"hardware_applied": false}),
        );
        let mut app = TuiApp::from_view(view_with_fans());
        app.socket = Some(socket);
        app.supervisor.state = SupervisorState::Healthy;
        app.tab = Tab::Profiles;
        app.profile_index = 1;
        app.handle_key(KeyCode::Char('p'));
        server.join().unwrap();
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("previewed; no fan write performed")
        );
    }

    #[test]
    fn dashboard_reports_active_profile_and_recovery_failure() {
        let mut app = TuiApp::from_view(view_with_fans());
        app.supervisor.status = Some(serde_json::json!({
            "profile": {
                "selected": "quiet",
                "active": true,
                "applied_fans": [0],
                "blocked_fans": [1]
            },
            "safety": {
                "state": "failsafe",
                "reasons": ["profile regulation stopped", "automatic recovery failed: backend auto failure"]
            }
        }));
        app.control = crate::diagnostics::control_readiness(&app.view);
        let text = render_size(&mut app, 140, 36);
        assert!(text.contains("Profile · Quiet"), "{text}");
        assert!(text.contains("applied [0]"), "{text}");
        assert!(text.contains("failsafe"), "{text}");
        assert!(text.contains("automatic recovery failed"), "{text}");
    }

    #[test]
    fn supervisor_unavailable_blocks_auto_and_profile_actions() {
        let mut app = TuiApp::from_view(empty_view());
        app.supervisor.state = SupervisorState::NotRunning;
        app.tab = Tab::Fans;
        app.handle_key(KeyCode::Char('A'));
        assert!(app.message.as_deref().unwrap_or("").contains("read-only"));
        app.tab = Tab::Profiles;
        app.handle_key(KeyCode::Char('d'));
        assert!(app.message.as_deref().unwrap_or("").contains("read-only"));
    }

    #[test]
    fn fan_panels_report_the_backend_reason_instead_of_a_false_disconnection() {
        let mut view = empty_view();
        view.supervisor.state = SupervisorState::Healthy;
        view.status = None;
        view.backend_error = Some("nbfc_service is not active".to_string());
        let mut app = TuiApp::from_view(view);

        app.tab = Tab::Dashboard;
        let text = render_size(&mut app, 110, 32);
        assert!(text.contains("Fan state unavailable"), "{text}");
        assert!(text.contains("nbfc_service is not active"), "{text}");
        assert!(!text.contains("not reachable"), "{text}");

        app.tab = Tab::Fans;
        let text = render_size(&mut app, 110, 32);
        assert!(text.contains("Fan discovery unavailable"), "{text}");
        assert!(text.contains("nbfc_service is not active"), "{text}");
    }

    #[test]
    fn long_fan_names_render_at_narrow_widths() {
        let mut view = view_with_fans();
        let long_name =
            "Very long logical fan name that must not overflow the terminal".to_string();
        view.status.as_mut().unwrap().fans[0].name = long_name.clone();
        view.sensor_config.as_mut().unwrap().fans[0].name = long_name;
        let mut app = TuiApp::from_view(view);
        app.tab = Tab::Fans;
        for (width, height) in [(40, 20), (80, 24), (120, 32)] {
            let text = render_size(&mut app, width, height);
            assert!(!text.is_empty(), "empty render at {width}x{height}");
        }
    }

    #[test]
    fn profile_selection_applies_immediately() {
        let mut app = TuiApp::from_view(view_with_fans());
        app.supervisor.state = SupervisorState::Healthy;
        app.socket = None;
        app.tab = Tab::Profiles;
        app.handle_key(KeyCode::Down);
        assert_eq!(app.profile_index, 1);
        app.handle_key(KeyCode::Enter);
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("No supervisor socket"),
            "{:?}",
            app.message
        );
    }

    #[test]
    fn profile_preview_without_socket_reports_clearly() {
        let mut app = TuiApp::from_view(view_with_fans());
        app.supervisor.state = SupervisorState::Healthy;
        app.socket = None;
        app.tab = Tab::Profiles;
        app.profile_index = 1; // quiet
        app.handle_key(KeyCode::Char('p'));
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("No supervisor socket"),
            "{:?}",
            app.message
        );
    }

    #[test]
    fn profile_table_shows_a_preview_target() {
        let mut app = TuiApp::from_view(empty_view());
        app.tab = Tab::Profiles;
        let text = render_size(&mut app, 100, 32);
        assert!(text.contains("Target"));
        assert!(text.contains("n/a") || text.contains('%'));
    }

    #[test]
    fn masthead_has_one_blank_top_margin_at_every_size() {
        for (width, height, ascii) in [
            (120, 40, false),
            (90, 30, false),
            (80, 24, false),
            (60, 18, false),
            (120, 40, true),
        ] {
            let mut app = TuiApp::from_view(view_with_two_fans());
            app.ascii_logo = ascii;
            let backend = ratatui::backend::TestBackend::new(width, height);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|frame| app.render(frame)).unwrap();
            let buffer = terminal.backend().buffer();
            let cells: Vec<String> = buffer
                .content()
                .iter()
                .map(|cell| cell.symbol().to_string())
                .collect();
            let first: String = cells.iter().take(width as usize).cloned().collect();
            let second: String = cells
                .iter()
                .skip(width as usize)
                .take(width as usize)
                .cloned()
                .collect();
            assert!(
                first.trim().is_empty(),
                "top margin missing at {width}x{height} ascii={ascii}: {first:?}"
            );
            assert!(
                !second.trim().is_empty(),
                "masthead missing after margin at {width}x{height}: {second:?}"
            );
        }
    }

    #[test]
    fn banner_header_renders_on_large_terminals() {
        let mut app = TuiApp::from_view(view_with_two_fans());
        let text = render_size(&mut app, 120, 40);
        assert!(text.contains(TAGLINE), "{text}");
        for row in LOGO_BANNER {
            assert!(text.contains(row.trim_end()), "missing banner row {row:?}");
        }
        assert!(text.contains("2.4.0"), "{text}");
        assert!(text.contains("NBFC"), "{text}");
    }

    #[test]
    fn ascii_banner_fallback_renders_without_unicode() {
        let mut app = TuiApp::from_view(view_with_two_fans());
        app.ascii_logo = true;
        let text = render_size(&mut app, 120, 40);
        assert!(text.contains(LOGO_BANNER_ASCII[0].trim_end()), "{text}");
        assert!(!text.contains("███╗"), "unicode banner leaked: {text}");
        assert!(text.contains(TAGLINE), "{text}");
    }

    #[test]
    fn resizing_switches_the_header_and_keeps_layout_stable() {
        let mut app = TuiApp::from_view(view_with_two_fans());
        let backend = ratatui::backend::TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        for (width, height, logo) in [
            (120, 40, true),
            (90, 30, true),
            (80, 24, false),
            (60, 18, false),
            (120, 40, true),
        ] {
            terminal.backend_mut().resize(width, height);
            terminal
                .resize(Rect::new(0, 0, width, height))
                .expect("resize");
            terminal.draw(|frame| app.render(frame)).unwrap();
            let text = buffer_text(&terminal);
            assert!(!text.is_empty(), "empty render at {width}x{height}");
            assert_eq!(
                text.contains(TAGLINE),
                logo,
                "header mode at {width}x{height}"
            );
        }
    }

    #[test]
    fn ascii_header_contains_no_non_ascii_glyphs() {
        let mut app = TuiApp::from_view(view_with_two_fans());
        app.ascii_logo = true;
        let backend = ratatui::backend::TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let header: String = buffer
            .content()
            .iter()
            .take(7 * 120)
            .map(ratatui::buffer::Cell::symbol)
            .collect();
        assert!(
            header.is_ascii(),
            "non-ASCII glyph in ASCII header: {header}"
        );
        assert!(header.contains("###+   ##+"), "{header}");
        assert!(header.contains("LINUX THERMAL MANAGEMENT"), "{header}");
        assert!(header.contains("| 2.4.0"), "{header}");
    }

    #[test]
    fn medium_header_uses_the_compact_wordmark() {
        let mut app = TuiApp::from_view(view_with_two_fans());
        let text = render_size(&mut app, 80, 24);
        assert!(text.contains(LOGO_MEDIUM[0].trim_end()), "{text}");
        assert!(text.contains("NBFC"), "{text}");
        assert!(!text.contains(TAGLINE), "{text}");
    }

    #[test]
    fn compact_header_replaces_the_wordmark_on_small_terminals() {
        let mut app = TuiApp::from_view(view_with_two_fans());
        let text = render_size(&mut app, 60, 18);
        assert!(!text.contains(LOGO_MEDIUM[0].trim_end()), "{text}");
        assert!(!text.contains(TAGLINE), "{text}");
        assert!(text.contains("NeoSense"), "{text}");
        assert!(text.contains("supervisor"), "{text}");
    }

    #[test]
    fn ascii_fallback_uses_the_compact_header_at_medium_sizes() {
        let mut app = TuiApp::from_view(view_with_two_fans());
        app.ascii_logo = true;
        let text = render_size(&mut app, 80, 24);
        assert!(!text.contains("█▄ █"), "{text}");
        assert!(text.contains("NeoSense"), "{text}");
    }

    #[test]
    fn header_never_leaks_html_entities_or_full_ipc_errors() {
        let mut app = TuiApp::from_view(empty_view());
        app.supervisor.state = SupervisorState::PermissionDenied;
        app.supervisor.error = Some("&#x20; very long IPC detail ".repeat(20));
        for width in [40, 80, 100, 160] {
            let text = render_size(&mut app, width, 32);
            assert!(!text.contains("&#x20;"), "HTML entity at width {width}");
            assert!(
                !text.contains("very long IPC detail"),
                "full IPC error leaked into header at width {width}"
            );
            assert!(
                text.contains("NeoSense")
                    || text.contains(TAGLINE)
                    || text.contains(LOGO_MEDIUM[0].trim_end())
                    || text.contains(LOGO_BANNER[0].trim_end()),
                "brand missing at width {width}: {text}"
            );
        }
    }

    #[test]
    fn empty_sensor_state_is_explicit() {
        let mut app = TuiApp::from_view(empty_view());
        app.tab = Tab::Sensors;
        let text = render_size(&mut app, 80, 24);
        assert!(text.contains("No sensors detected"));
    }

    #[test]
    fn scroll_hint_is_visible_on_paragraph_views() {
        let mut app = TuiApp::from_view(empty_view());
        app.tab = Tab::System;
        let text = render_size(&mut app, 80, 24);
        assert!(text.contains("j/k"), "{text}");
        let text = render_size(&mut app, 100, 32);
        assert!(text.contains("PgUp/PgDn"), "{text}");
    }

    #[test]
    fn manual_target_confirmation_is_visible_in_the_footer() {
        let mut app = TuiApp::from_view(empty_view());
        app.tab = Tab::Fans;
        app.pending_fan_set = Some((0, 60.0));
        app.message = Some(
            "Request fan 0 target 60.0%? Manual mode is not temperature regulation. y/n"
                .to_string(),
        );
        let text = render_size(&mut app, 100, 32);
        assert!(text.contains("y / n"), "{text}");
        assert!(text.contains("Request fan 0 target"), "{text}");
    }

    #[test]
    fn action_message_is_visible_in_the_footer() {
        let mut app = TuiApp::from_view(empty_view());
        app.message = Some("NBFC automatic mode confirmed".to_string());
        let text = render_size(&mut app, 80, 24);
        assert!(text.contains("NBFC automatic mode confirmed"));
    }

    #[test]
    fn footer_truncation_keeps_short_messages() {
        assert_eq!(truncate_for_width("short", 10), "short");
        assert_eq!(truncate_for_width("abcdefgh", 6), "abc...");
        assert_eq!(truncate_for_width("abcdefgh", 2), "..");
    }

    #[test]
    fn compact_paragraph_views_advertise_scrolling() {
        let mut app = TuiApp::from_view(empty_view());
        app.tab = Tab::System;
        let text = render_size(&mut app, 40, 20);
        assert!(text.contains("j/k"));
    }

    #[test]
    fn presets_render_and_apply_without_confirmation() {
        let mut app = TuiApp::from_view(empty_view());
        app.tab = Tab::Presets;
        let text = render_size(&mut app, 80, 24);
        assert!(text.contains("Silent"), "{text}");
        assert!(text.contains("Balanced"), "{text}");
        assert!(text.contains("Maximum"), "{text}");
        assert!(text.contains("25%"), "{text}");
        assert!(text.contains("100%"), "{text}");
        app.handle_key(KeyCode::Enter);
        assert!(
            app.message.as_deref().unwrap_or("").contains("read-only"),
            "{:?}",
            app.message
        );
    }

    #[test]
    fn preset_selection_sends_one_apply_request() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("tui-preset.sock");
        let server = serve_once(
            socket.clone(),
            "preset.apply",
            |request| {
                assert_eq!(request.params["name"], "silent");
                assert_eq!(request.params["confirm"], true);
            },
            serde_json::json!({"hardware_applied": true, "verified": true}),
        );
        let mut app = TuiApp::from_view(view_with_fans());
        app.socket = Some(socket);
        app.supervisor.state = SupervisorState::Healthy;
        app.tab = Tab::Presets;
        app.preset_index = 0;
        app.handle_key(KeyCode::Enter);
        server.join().unwrap();
        let message = app.message.as_deref().unwrap_or("");
        assert!(message.contains("Silent applied"), "{message}");
        assert!(message.contains("CPU 25%"), "{message}");
    }

    #[test]
    fn active_preset_is_inferred_and_highlighted() {
        let mut app = TuiApp::from_view(view_with_two_fans());
        let expires = crate::model::now_ms() + 900_000;
        app.supervisor.status = Some(serde_json::json!({
            "profile": {"active": false},
            "safety": {
                "state": "manual",
                "manual": null,
                "preset": [
                    {"fan_index": 0, "percent": 60.0, "expires_at_ms": expires},
                    {"fan_index": 1, "percent": 60.0, "expires_at_ms": expires}
                ]
            }
        }));
        assert_eq!(app.operating_mode(), OperatingMode::Preset(1));
        let dashboard = render_size(&mut app, 120, 36);
        assert!(
            dashboard.contains("Balanced · fixed targets"),
            "{dashboard}"
        );
        assert!(dashboard.contains("MANUAL"), "{dashboard}");

        app.select_tab(Tab::Presets);
        assert_eq!(app.preset_index, 1);
        let presets = render_size(&mut app, 120, 34);
        assert!(presets.contains("● ACTIVE"), "{presets}");
    }

    #[test]
    fn both_fans_row_is_selectable_and_editable() {
        let mut app = TuiApp::from_view(view_with_two_fans());
        app.supervisor.state = SupervisorState::Healthy;
        app.tab = Tab::Fans;
        assert!(app.both_available());
        app.handle_key(KeyCode::Down);
        app.handle_key(KeyCode::Down);
        assert!(app.selection_is_both());
        app.handle_key(KeyCode::Char('m'));
        assert!(app.editing_both.is_some());
        app.handle_key(KeyCode::Esc);
        app.handle_key(KeyCode::Char('+'));
        assert!(app.pending_both_set.is_some());
        app.handle_key(KeyCode::Char('n'));
        assert!(app.pending_both_set.is_none());
    }

    #[test]
    fn active_profile_is_highlighted_in_the_profile_view() {
        let mut app = TuiApp::from_view(view_with_fans());
        app.supervisor.status = Some(serde_json::json!({
            "profile": {
                "selected": "quiet",
                "active": true,
                "applied_fans": [0],
                "blocked_fans": []
            },
            "safety": {"state": "profile", "reasons": []}
        }));
        app.tab = Tab::Profiles;
        let text = render_size(&mut app, 120, 34);
        assert!(text.contains("● Quiet active"), "{text}");
    }
    fn custom_status_value(name: &str) -> Value {
        serde_json::json!({
            "custom": {
                "schema": "neosense.custom_profiles_status/v1",
                "path": "/tmp/test-profiles.json",
                "profiles": [{
                    "name": name,
                    "title": "Custom test",
                    "description": "test",
                    "points": [
                        {"temperature_c": 40.0, "percent": 25.0},
                        {"temperature_c": 80.0, "percent": 80.0}
                    ],
                    "min_percent": 20.0,
                    "max_percent": 100.0,
                    "hysteresis_c": 3.0,
                    "max_step_percent": 10.0,
                    "stale_after_ms": 3000,
                    "fan_roles": ["cpu"]
                }],
                "warnings": [],
                "error": null,
                "recovered_from_backup": false
            }
        })
    }

    #[test]
    fn custom_profiles_are_inspected_in_the_profile_view() {
        let mut app = TuiApp::from_view(empty_view());
        app.supervisor.status = Some(custom_status_value("mine"));
        app.tab = Tab::Profiles;
        let text = render_size(&mut app, 100, 30);
        assert!(text.contains("mine"));
        assert!(text.contains("custom"));
        let evaluations = app.evaluations();
        let index = evaluations
            .iter()
            .position(|evaluation| evaluation.profile.name == "mine")
            .unwrap();
        app.profile_index = index;
        app.tab = Tab::System;
        let system = render_size(&mut app, 100, 30);
        assert!(system.contains("1 profile(s)"), "{system}");
        app.tab = Tab::Profiles;
        app.handle_key(KeyCode::Char('e'));
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("through the supervisor")
        );
    }

    #[test]
    fn new_custom_curve_opens_a_valid_editor_template() {
        let mut app = TuiApp::from_view(empty_view());
        app.socket = Some(PathBuf::from("/nonexistent-neosense-test.sock"));
        app.supervisor.state = SupervisorState::Connected;
        app.begin_new_custom();
        let request = app.pending_editor.clone().expect("editor request");
        let profile: CustomProfile =
            serde_json::from_str(&std::fs::read_to_string(&request.path).unwrap()).unwrap();
        assert!(profile.validate().is_ok());
        assert_eq!(profile.name, "my-curve");
        assert!(matches!(request.action, EditorAction::Create));
        let _ = std::fs::remove_file(request.path);
    }

    #[test]
    fn edited_custom_curve_is_validated_and_sent_through_ipc() {
        use crate::ipc::{Request, Response, read_message, write_message};
        use std::io::BufReader;
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("tui-edit.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(&stream);
            let request: Request = read_message(&mut reader).unwrap();
            assert_eq!(request.method, "custom.create");
            assert_eq!(request.params["profile"]["name"], "edited");
            let mut stream = stream;
            write_message(
                &mut stream,
                &Response::ok(request.id, serde_json::json!({"ok": true})),
            )
            .unwrap();
        });

        let path = dir.path().join("edited.json");
        let profile = CustomProfile {
            name: "edited".to_string(),
            title: "Edited".to_string(),
            description: String::new(),
            points: vec![
                crate::regulation::CurvePoint {
                    temperature_c: 40.0,
                    percent: 25.0,
                },
                crate::regulation::CurvePoint {
                    temperature_c: 80.0,
                    percent: 80.0,
                },
            ],
            min_percent: 20.0,
            max_percent: 100.0,
            hysteresis_c: 3.0,
            max_step_percent: 10.0,
            stale_after_ms: 3_000,
            fan_roles: vec![FanRole::Cpu],
        };
        std::fs::write(&path, serde_json::to_string(&profile).unwrap()).unwrap();

        let mut app = TuiApp::from_view(empty_view());
        app.socket = Some(socket);
        app.supervisor.state = SupervisorState::Connected;
        app.finish_editor(EditorRequest {
            path: path.clone(),
            action: EditorAction::Create,
        });
        server.join().unwrap();
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("created through the supervisor"),
            "{:?}",
            app.message
        );
        assert!(!path.exists());
    }

    #[test]
    fn invalid_edited_curve_is_not_sent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.json");
        std::fs::write(&path, b"{not json").unwrap();
        let mut app = TuiApp::from_view(empty_view());
        app.socket = Some(dir.path().join("unused.sock"));
        app.finish_editor(EditorRequest {
            path: path.clone(),
            action: EditorAction::Create,
        });
        assert!(
            app.message
                .as_deref()
                .unwrap_or("")
                .contains("not valid JSON")
        );
        assert!(!path.exists());
    }

    #[test]
    fn active_and_selected_markers_use_state_styles() {
        let mut app = TuiApp::from_view(view_with_two_fans());
        let expires = crate::model::now_ms() + 900_000;
        app.supervisor.status = Some(serde_json::json!({
            "profile": {"active": false},
            "safety": {
                "state": "manual",
                "preset": [
                    {"fan_index": 0, "percent": 25.0, "expires_at_ms": expires},
                    {"fan_index": 1, "percent": 25.0, "expires_at_ms": expires}
                ]
            }
        }));
        app.tab = Tab::Presets;
        app.preset_index = 1;
        let backend = ratatui::backend::TestBackend::new(120, 34);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let cells: Vec<(String, Style)> = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| (cell.symbol().to_string(), cell.style()))
            .collect();
        assert!(
            cells
                .iter()
                .any(|(symbol, style)| symbol == "●" && style.fg == Some(theme::ACTIVE)),
            "active preset must carry the green marker"
        );
        assert!(
            cells
                .iter()
                .any(|(symbol, style)| symbol == "▶" && style.fg == Some(theme::ACCENT)),
            "selected preset must carry the accent marker"
        );
    }

    #[test]
    fn active_profile_focus_uses_an_accent_or_green_outline() {
        let mut app = TuiApp::from_view(view_with_fans());
        app.supervisor.status = Some(serde_json::json!({
            "profile": {
                "selected": "quiet",
                "active": true,
                "applied_fans": [0],
                "blocked_fans": []
            },
            "safety": {"state": "profile", "reasons": []}
        }));
        app.tab = Tab::Profiles;
        let backend = ratatui::backend::TestBackend::new(120, 34);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let has_state_border = buffer.content().iter().any(|cell| {
            matches!(cell.style().fg, Some(color) if color == theme::ACTIVE || color == theme::ACCENT)
                && cell.symbol() == "─"
        });
        assert!(
            has_state_border,
            "active profile panel needs a state border"
        );
    }

    #[test]
    fn help_overlay_scrolls_in_small_terminals() {
        let mut app = TuiApp::from_view(empty_view());
        app.handle_key(KeyCode::Char('?'));
        assert!(app.show_help);
        for _ in 0..3 {
            app.handle_key(KeyCode::Down);
        }
        assert_eq!(app.help_scroll, 3);
        let text = render_size(&mut app, 60, 18);
        assert!(text.contains("Help"), "{text}");
        app.handle_key(KeyCode::Esc);
        assert!(!app.show_help);
        assert_eq!(app.help_scroll, 0);
    }

    #[test]
    fn help_overlay_renders() {
        let mut app = TuiApp::from_view(empty_view());
        app.show_help = true;
        let text = render_once(&mut app);
        assert!(text.contains("NeoSense — help"));
        assert!(text.contains("quit"));
    }
}
