use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use tauri::image::Image;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, Submenu};
use tauri::path::BaseDirectory;
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, LogicalPosition, Manager, Monitor, Rect, Runtime};

use crate::adapters::source::{SourceHookStatus, TokenSourceKind};
use crate::app::menu::menu_labels;
use crate::app::state::{AppState, MenuAction, MenuActionOutcome};
use crate::app::widget_events::PROFILE_WINDOW_FOCUSED_EVENT;
use crate::core::pricing::CostPeriodSummary;

pub const TRAY_REFRESH_FALLBACK_INTERVAL: Duration = Duration::from_secs(300);

pub fn tray_title_from_total(tokens: i64) -> String {
    if tokens >= 1_000_000 {
        let value = (tokens as f64 / 100_000.0).round() / 10.0;
        format!("{value:.1}M")
    } else if tokens >= 1_000 {
        format!("{}K", (tokens as f64 / 1000.0).round() as i64)
    } else {
        tokens.to_string()
    }
}

pub fn tray_amount_from_cost(value: f64) -> String {
    let value = if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    };
    if value < 10_000.0 {
        format!("¥{value:.2}")
    } else if value < 1_000_000.0 {
        let compact = (value / 100.0).round() / 10.0;
        format!("¥{compact:.1}k")
    } else {
        let compact = (value / 100_000.0).round() / 10.0;
        format!("¥{compact:.1}M")
    }
}

pub fn tray_title_from_cost_summary(summary: &CostPeriodSummary) -> String {
    format!(
        " {} · {}",
        tray_amount_from_cost(summary.estimated_cost),
        tray_title_from_total(summary.total_tokens)
    )
}

fn current_tray_title(state: &AppState) -> String {
    let now_utc = chrono::Utc::now();
    let now_local = now_utc.with_timezone(&chrono::Local);
    state
        .widget_cost_summary_at(now_utc, now_local)
        .map(|summary| tray_title_from_cost_summary(&summary.today))
        .unwrap_or_else(|_| {
            tray_title_from_total(
                state
                    .widget_state_at(chrono::Local::now())
                    .today_total_tokens,
            )
        })
}

pub fn tray_icon_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("icons/tray-icon.png")
}

pub fn tray_icon_resource_path<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<PathBuf> {
    app.path()
        .resolve("icons/tray-icon.png", BaseDirectory::Resource)
}

fn tray_icon<R: Runtime>(app: &AppHandle<R>) -> Option<Image<'static>> {
    tray_icon_resource_path(app)
        .ok()
        .and_then(|path| Image::from_path(path).ok())
        .or_else(|| Image::from_path(tray_icon_path()).ok())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceMenuItemModel {
    pub source: TokenSourceKind,
    pub id: &'static str,
    pub label: &'static str,
    pub checked: bool,
}

pub fn source_menu_item_models(statuses: &[SourceHookStatus]) -> Vec<SourceMenuItemModel> {
    TokenSourceKind::all_menu_sources()
        .into_iter()
        .map(|source| {
            let checked = statuses
                .iter()
                .find(|status| status.source == source)
                .is_some_and(|status| status.hook_registered);
            SourceMenuItemModel {
                source,
                id: MenuAction::ToggleSourceHook(source).to_menu_id(),
                label: source.display_name(),
                checked,
            }
        })
        .collect()
}

pub fn menu_action_from_id(id: &str) -> Option<MenuAction> {
    match id {
        "toggle_source_traex_hook" => Some(MenuAction::ToggleSourceHook(TokenSourceKind::Traex)),
        "toggle_source_codex_hook" => Some(MenuAction::ToggleSourceHook(TokenSourceKind::Codex)),
        "toggle_source_claude_hook" => Some(MenuAction::ToggleSourceHook(TokenSourceKind::Claude)),
        "toggle_source_cursor_hook" => Some(MenuAction::ToggleSourceHook(TokenSourceKind::Cursor)),
        "pause_tracking" => Some(MenuAction::PauseTracking),
        "resume_tracking" => Some(MenuAction::ResumeTracking),
        "open_logs" => Some(MenuAction::OpenLogs),
        "copy_debug_bundle" => Some(MenuAction::CopyDebugBundle),
        "enable_debug_logging" => Some(MenuAction::EnableDebugLogging),
        "quit" => Some(MenuAction::Quit),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuActionRefreshTrigger {
    ActionHandled,
    ActionFailed,
}

pub fn should_refresh_tray_menu_after_action(
    action: MenuAction,
    trigger: MenuActionRefreshTrigger,
) -> bool {
    matches!(
        (action, trigger),
        (
            MenuAction::ToggleSourceHook(_),
            MenuActionRefreshTrigger::ActionHandled | MenuActionRefreshTrigger::ActionFailed
        )
    )
}

pub fn handle_menu_action_outcome<R: Runtime>(
    app: &AppHandle<R>,
    outcome: MenuActionOutcome,
) -> anyhow::Result<()> {
    match outcome {
        MenuActionOutcome::LogsDirectoryRequested(path) => open_logs_directory(&path),
        MenuActionOutcome::DebugBundleCreated(path) => copy_debug_bundle_path_to_pasteboard(&path),
        MenuActionOutcome::QuitRequested => {
            app.exit(0);
            Ok(())
        }
        _ => Ok(()),
    }
}

const PROFILE_WINDOW_WIDTH: f64 = 428.0;
const PROFILE_WINDOW_HEIGHT: f64 = 572.0;
const PROFILE_WINDOW_GAP: f64 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct ProfileWindowSize {
    width: f64,
    height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ProfileTrayRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ProfileMonitorBounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ProfileMonitorGeometry {
    physical_bounds: ProfileMonitorBounds,
    logical_bounds: ProfileMonitorBounds,
    scale_factor: f64,
}

impl ProfileMonitorBounds {
    fn contains(self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

fn profile_window_position(
    rect: ProfileTrayRect,
    scale_factor: f64,
    window_size: ProfileWindowSize,
    monitor_bounds: Option<ProfileMonitorBounds>,
) -> LogicalPosition<f64> {
    let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    let rect = ProfileTrayRect {
        x: rect.x / scale_factor,
        y: rect.y / scale_factor,
        width: rect.width / scale_factor,
        height: rect.height / scale_factor,
    };
    let anchor_x = rect.x + rect.width / 2.0;
    let anchor_y = rect.y + rect.height;
    let mut x = anchor_x - window_size.width / 2.0;
    let mut y = anchor_y + PROFILE_WINDOW_GAP;

    if let Some(bounds) = monitor_bounds {
        x = clamp_to_range(x, bounds.x, bounds.x + bounds.width - window_size.width);
        y = clamp_to_range(y, bounds.y, bounds.y + bounds.height - window_size.height);
    }

    LogicalPosition::new(x, y)
}

fn profile_window_monitor_bounds(
    rect: ProfileTrayRect,
    monitors: &[ProfileMonitorGeometry],
    fallback: Option<ProfileMonitorGeometry>,
) -> Option<ProfileMonitorGeometry> {
    let anchor_x = rect.x + rect.width / 2.0;
    let anchor_y = rect.y + rect.height / 2.0;

    monitors
        .iter()
        .copied()
        .find(|monitor| monitor.physical_bounds.contains(anchor_x, anchor_y))
        .or(fallback)
}

fn profile_window_selected_monitor<E>(
    rect: ProfileTrayRect,
    available_monitors: Result<Vec<ProfileMonitorGeometry>, E>,
    fallback: Option<ProfileMonitorGeometry>,
) -> Option<ProfileMonitorGeometry> {
    match available_monitors {
        Ok(monitors) => profile_window_monitor_bounds(rect, &monitors, fallback),
        Err(_) => None,
    }
}

#[cfg(test)]
fn profile_monitor_geometry(
    physical_bounds: ProfileMonitorBounds,
    scale_factor: f64,
) -> ProfileMonitorGeometry {
    profile_monitor_geometry_from_bounds(physical_bounds, physical_bounds, scale_factor)
}

fn profile_monitor_geometry_from_bounds(
    physical_bounds: ProfileMonitorBounds,
    work_area_bounds: ProfileMonitorBounds,
    scale_factor: f64,
) -> ProfileMonitorGeometry {
    let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    ProfileMonitorGeometry {
        physical_bounds,
        logical_bounds: ProfileMonitorBounds {
            x: work_area_bounds.x / scale_factor,
            y: work_area_bounds.y / scale_factor,
            width: work_area_bounds.width / scale_factor,
            height: work_area_bounds.height / scale_factor,
        },
        scale_factor,
    }
}

fn profile_tray_rect_from_tauri(rect: Rect, scale_factor: f64) -> ProfileTrayRect {
    let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    let position = rect.position.to_physical::<f64>(scale_factor);
    let size = rect.size.to_physical::<f64>(scale_factor);
    ProfileTrayRect {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    }
}

fn profile_monitor_geometry_from_tauri(monitor: &Monitor) -> ProfileMonitorGeometry {
    let position = monitor.position();
    let size = monitor.size();
    let work_area = monitor.work_area();
    profile_monitor_geometry_from_bounds(
        ProfileMonitorBounds {
            x: position.x as f64,
            y: position.y as f64,
            width: size.width as f64,
            height: size.height as f64,
        },
        ProfileMonitorBounds {
            x: work_area.position.x as f64,
            y: work_area.position.y as f64,
            width: work_area.size.width as f64,
            height: work_area.size.height as f64,
        },
        monitor.scale_factor(),
    )
}

fn clamp_to_range(value: f64, min: f64, max: f64) -> f64 {
    if max < min {
        min
    } else {
        value.clamp(min, max)
    }
}

pub fn show_profile_window_near_tray<R: Runtime>(
    app: &AppHandle<R>,
    rect: Rect,
) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("main") {
        let fallback_scale_factor = window.scale_factor().unwrap_or(1.0);
        let tray_rect = profile_tray_rect_from_tauri(rect, fallback_scale_factor);
        let available_monitors = window.available_monitors().map(|monitors| {
            monitors
                .into_iter()
                .map(|monitor| profile_monitor_geometry_from_tauri(&monitor))
                .collect::<Vec<_>>()
        });
        let fallback_monitor = window
            .current_monitor()
            .ok()
            .flatten()
            .map(|monitor| profile_monitor_geometry_from_tauri(&monitor));
        let selected_monitor =
            profile_window_selected_monitor(tray_rect, available_monitors, fallback_monitor);
        let (scale_factor, monitor_bounds) = selected_monitor
            .map(|monitor| (monitor.scale_factor, Some(monitor.logical_bounds)))
            .unwrap_or((fallback_scale_factor, None));
        let position = profile_window_position(
            tray_rect,
            scale_factor,
            ProfileWindowSize {
                width: PROFILE_WINDOW_WIDTH,
                height: PROFILE_WINDOW_HEIGHT,
            },
            monitor_bounds,
        );
        window.set_position(position)?;
        window.show()?;
        window.set_focus()?;
        let _ = app.emit(PROFILE_WINDOW_FOCUSED_EVENT, ());
    }
    Ok(())
}

pub fn build_tray_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let labels = menu_labels();
    let source_statuses = {
        let state = app.state::<AppState>();
        state.source_hook_statuses()
    };
    let source_models = source_menu_item_models(&source_statuses);
    let source_items = source_models
        .iter()
        .map(|model| {
            CheckMenuItem::with_id(
                app,
                model.id,
                model.label,
                true,
                model.checked,
                None::<&str>,
            )
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    let source_refs = source_items
        .iter()
        .map(|item| item as &dyn tauri::menu::IsMenuItem<R>)
        .collect::<Vec<_>>();
    let sources = Submenu::with_items(app, labels.source_submenu, true, &source_refs)?;

    let pause = MenuItem::with_id(
        app,
        MenuAction::PauseTracking.to_menu_id(),
        labels.pause_tracking,
        true,
        None::<&str>,
    )?;
    let resume = MenuItem::with_id(
        app,
        MenuAction::ResumeTracking.to_menu_id(),
        labels.resume_tracking,
        true,
        None::<&str>,
    )?;
    let open_logs = MenuItem::with_id(
        app,
        MenuAction::OpenLogs.to_menu_id(),
        labels.open_logs,
        true,
        None::<&str>,
    )?;
    let copy_debug = MenuItem::with_id(
        app,
        MenuAction::CopyDebugBundle.to_menu_id(),
        labels.copy_debug_bundle,
        true,
        None::<&str>,
    )?;
    let enable_debug = MenuItem::with_id(
        app,
        MenuAction::EnableDebugLogging.to_menu_id(),
        labels.enable_debug_logging,
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(
        app,
        MenuAction::Quit.to_menu_id(),
        labels.quit,
        true,
        None::<&str>,
    )?;

    Menu::with_items(
        app,
        &[
            &sources,
            &pause,
            &resume,
            &open_logs,
            &copy_debug,
            &enable_debug,
            &quit,
        ],
    )
}

pub fn rebuild_tray_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    build_tray_menu(app)
}

fn refresh_tray_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if let Some(tray) = app.tray_by_id("token-fire") {
        let menu = rebuild_tray_menu(app)?;
        tray.set_menu(Some(menu))?;
    }
    Ok(())
}

pub fn install_tray<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<TrayIcon<R>> {
    let menu = build_tray_menu(app)?;
    let title = {
        let state = app.state::<AppState>();
        current_tray_title(&state)
    };
    let mut builder = TrayIconBuilder::with_id("token-fire")
        .title(title)
        .menu(&menu)
        .icon_as_template(false)
        .show_menu_on_left_click(false);
    if let Some(icon) = tray_icon(app) {
        builder = builder.icon(icon);
    }
    builder
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                let _ = show_profile_window_near_tray(tray.app_handle(), rect);
            }
        })
        .on_menu_event(|app, event| {
            let Some(action) = menu_action_from_id(event.id().as_ref()) else {
                return;
            };
            let state = app.state::<AppState>();
            match state.handle_menu_action(action) {
                Ok(outcome) => {
                    if let Err(error) = handle_menu_action_outcome(app, outcome) {
                        eprintln!("failed to handle tray menu action outcome: {error:#}");
                    }
                    if should_refresh_tray_menu_after_action(
                        action,
                        MenuActionRefreshTrigger::ActionHandled,
                    ) {
                        if let Err(error) = refresh_tray_menu(app) {
                            eprintln!("failed to refresh tray menu: {error:#}");
                        }
                    }
                }
                Err(error) => {
                    eprintln!("failed to handle tray menu action: {error:#}");
                    if should_refresh_tray_menu_after_action(
                        action,
                        MenuActionRefreshTrigger::ActionFailed,
                    ) {
                        if let Err(error) = refresh_tray_menu(app) {
                            eprintln!("failed to refresh tray menu: {error:#}");
                        }
                    }
                }
            }
        })
        .build(app)
}

pub fn refresh_tray_title<R: Runtime>(tray: &TrayIcon<R>, state: &AppState) -> tauri::Result<()> {
    let title = current_tray_title(state);
    tray.set_title(Some(&title))
}

pub fn refresh_tray_title_from_app<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let Some(tray) = app.tray_by_id("token-fire") else {
        return Ok(());
    };
    let state = app.state::<AppState>();
    refresh_tray_title(&tray, &state)
}

pub fn open_logs_directory(path: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(path)?;
    Command::new("open").arg(path).spawn()?;
    Ok(())
}

pub fn copy_debug_bundle_path_with(
    path: &Path,
    mut copy: impl FnMut(&str) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    copy(&path.to_string_lossy())
}

pub fn copy_debug_bundle_path_to_pasteboard(path: &Path) -> anyhow::Result<()> {
    copy_debug_bundle_path_with(path, |text| {
        let mut child = Command::new("pbcopy").stdin(Stdio::piped()).spawn()?;
        let Some(stdin) = child.stdin.as_mut() else {
            anyhow::bail!("pbcopy stdin unavailable");
        };
        stdin.write_all(text.as_bytes())?;
        let status = child.wait()?;
        if !status.success() {
            anyhow::bail!("pbcopy exited with {status}");
        }
        Ok(())
    })
}

pub fn start_tray_refresh_loop<R: Runtime>(app: AppHandle<R>) -> thread::JoinHandle<()> {
    thread::spawn(move || loop {
        thread::sleep(TRAY_REFRESH_FALLBACK_INTERVAL);
        let _ = refresh_tray_title_from_app(&app);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_window_size() -> ProfileWindowSize {
        ProfileWindowSize {
            width: PROFILE_WINDOW_WIDTH,
            height: PROFILE_WINDOW_HEIGHT,
        }
    }

    #[test]
    fn profile_window_position_centers_under_tray_rect_without_monitor_bounds() {
        let position = profile_window_position(
            ProfileTrayRect {
                x: 1000.0,
                y: 20.0,
                width: 24.0,
                height: 22.0,
            },
            1.0,
            default_window_size(),
            None,
        );

        assert_eq!(position.x, 798.0);
        assert_eq!(position.y, 50.0);
    }

    #[test]
    fn profile_window_position_converts_physical_rect_to_logical_coordinates() {
        let position = profile_window_position(
            ProfileTrayRect {
                x: 2000.0,
                y: 40.0,
                width: 48.0,
                height: 44.0,
            },
            2.0,
            default_window_size(),
            None,
        );

        assert_eq!(position.x, 798.0);
        assert_eq!(position.y, 50.0);
    }

    #[test]
    fn profile_window_position_clamps_to_right_monitor_edge() {
        let position = profile_window_position(
            ProfileTrayRect {
                x: 1380.0,
                y: 20.0,
                width: 24.0,
                height: 22.0,
            },
            1.0,
            default_window_size(),
            Some(ProfileMonitorBounds {
                x: 0.0,
                y: 0.0,
                width: 1440.0,
                height: 900.0,
            }),
        );

        assert_eq!(position.x, 1012.0);
        assert_eq!(position.y, 50.0);
    }

    #[test]
    fn profile_window_position_clamps_to_left_monitor_edge() {
        let position = profile_window_position(
            ProfileTrayRect {
                x: 8.0,
                y: 20.0,
                width: 24.0,
                height: 22.0,
            },
            1.0,
            default_window_size(),
            Some(ProfileMonitorBounds {
                x: 0.0,
                y: 0.0,
                width: 1440.0,
                height: 900.0,
            }),
        );

        assert_eq!(position.x, 0.0);
        assert_eq!(position.y, 50.0);
    }

    #[test]
    fn profile_window_position_clamps_to_bottom_monitor_edge() {
        let position = profile_window_position(
            ProfileTrayRect {
                x: 1000.0,
                y: 850.0,
                width: 24.0,
                height: 22.0,
            },
            1.0,
            default_window_size(),
            Some(ProfileMonitorBounds {
                x: 0.0,
                y: 0.0,
                width: 1440.0,
                height: 900.0,
            }),
        );

        assert_eq!(position.x, 798.0);
        assert_eq!(position.y, 328.0);
    }

    #[test]
    fn profile_window_monitor_bounds_prefers_monitor_containing_tray_anchor() {
        let left = profile_monitor_geometry(
            ProfileMonitorBounds {
                x: -1440.0,
                y: 0.0,
                width: 1440.0,
                height: 900.0,
            },
            1.0,
        );
        let right = profile_monitor_geometry(
            ProfileMonitorBounds {
                x: 0.0,
                y: 0.0,
                width: 1440.0,
                height: 900.0,
            },
            1.0,
        );

        let selected = profile_window_monitor_bounds(
            ProfileTrayRect {
                x: -1200.0,
                y: 20.0,
                width: 24.0,
                height: 22.0,
            },
            &[right, left],
            Some(right),
        );

        assert_eq!(selected, Some(left));
    }

    #[test]
    fn profile_window_monitor_bounds_uses_fallback_when_anchor_matches_no_monitor() {
        let fallback = profile_monitor_geometry(
            ProfileMonitorBounds {
                x: 0.0,
                y: 0.0,
                width: 1440.0,
                height: 900.0,
            },
            1.0,
        );

        let selected = profile_window_monitor_bounds(
            ProfileTrayRect {
                x: 4000.0,
                y: 20.0,
                width: 24.0,
                height: 22.0,
            },
            &[fallback],
            Some(fallback),
        );

        assert_eq!(selected, Some(fallback));
    }

    #[test]
    fn profile_window_monitor_bounds_treats_right_edge_as_next_monitor() {
        let left = profile_monitor_geometry(
            ProfileMonitorBounds {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
            1.0,
        );
        let right = profile_monitor_geometry(
            ProfileMonitorBounds {
                x: 100.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
            1.0,
        );

        let selected = profile_window_monitor_bounds(
            ProfileTrayRect {
                x: 90.0,
                y: 10.0,
                width: 20.0,
                height: 20.0,
            },
            &[left, right],
            None,
        );

        assert_eq!(selected, Some(right));
    }

    #[test]
    fn profile_window_selection_skips_fallback_when_available_monitors_fail() {
        let fallback = profile_monitor_geometry(
            ProfileMonitorBounds {
                x: 0.0,
                y: 0.0,
                width: 1440.0,
                height: 900.0,
            },
            1.0,
        );

        let selected = profile_window_selected_monitor(
            ProfileTrayRect {
                x: 100.0,
                y: 20.0,
                width: 24.0,
                height: 22.0,
            },
            Err(()),
            Some(fallback),
        );

        assert_eq!(selected, None);
    }

    #[test]
    fn profile_window_position_uses_selected_monitor_scale_for_mixed_dpi_tray_anchor() {
        let standard_monitor = profile_monitor_geometry(
            ProfileMonitorBounds {
                x: 0.0,
                y: 0.0,
                width: 1440.0,
                height: 900.0,
            },
            1.0,
        );
        let retina_monitor = profile_monitor_geometry(
            ProfileMonitorBounds {
                x: 1440.0,
                y: 0.0,
                width: 2880.0,
                height: 1800.0,
            },
            2.0,
        );
        let tray_rect = ProfileTrayRect {
            x: 2200.0,
            y: 40.0,
            width: 48.0,
            height: 44.0,
        };

        let selected = profile_window_monitor_bounds(
            tray_rect,
            &[standard_monitor, retina_monitor],
            Some(standard_monitor),
        )
        .expect("tray anchor should match a monitor");
        assert_eq!(selected, retina_monitor);

        let position = profile_window_position(
            tray_rect,
            selected.scale_factor,
            default_window_size(),
            Some(selected.logical_bounds),
        );

        assert_eq!(position.x, 898.0);
        assert_eq!(position.y, 50.0);
    }

    #[test]
    fn profile_window_monitor_bounds_selects_monitor_from_full_bounds_not_work_area() {
        let standard_monitor = profile_monitor_geometry_from_bounds(
            ProfileMonitorBounds {
                x: 0.0,
                y: 0.0,
                width: 1440.0,
                height: 900.0,
            },
            ProfileMonitorBounds {
                x: 0.0,
                y: 25.0,
                width: 1440.0,
                height: 875.0,
            },
            1.0,
        );
        let retina_monitor = profile_monitor_geometry_from_bounds(
            ProfileMonitorBounds {
                x: 1440.0,
                y: 0.0,
                width: 2880.0,
                height: 1800.0,
            },
            ProfileMonitorBounds {
                x: 1440.0,
                y: 50.0,
                width: 2880.0,
                height: 1750.0,
            },
            2.0,
        );
        let tray_rect = ProfileTrayRect {
            x: 2200.0,
            y: 0.0,
            width: 48.0,
            height: 22.0,
        };
        let anchor_x = tray_rect.x + tray_rect.width / 2.0;
        let anchor_y = tray_rect.y + tray_rect.height / 2.0;
        let retina_work_area_in_physical = ProfileMonitorBounds {
            x: retina_monitor.logical_bounds.x * retina_monitor.scale_factor,
            y: retina_monitor.logical_bounds.y * retina_monitor.scale_factor,
            width: retina_monitor.logical_bounds.width * retina_monitor.scale_factor,
            height: retina_monitor.logical_bounds.height * retina_monitor.scale_factor,
        };

        assert!(retina_monitor.physical_bounds.contains(anchor_x, anchor_y));
        assert!(!retina_work_area_in_physical.contains(anchor_x, anchor_y));
        assert_eq!(
            retina_monitor.logical_bounds,
            ProfileMonitorBounds {
                x: 720.0,
                y: 25.0,
                width: 1440.0,
                height: 875.0,
            }
        );

        let selected = profile_window_monitor_bounds(
            tray_rect,
            &[standard_monitor, retina_monitor],
            Some(standard_monitor),
        )
        .expect("tray anchor should match the full retina monitor bounds");

        assert_eq!(selected, retina_monitor);
    }
}
