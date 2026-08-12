use serde::Serialize;
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{
  AppHandle, Emitter, Manager, LogicalPosition, LogicalSize, Url, Webview, WebviewUrl,
  webview::{PageLoadEvent, WebviewBuilder},
};
use xcoding_providers::user_config_dir;

// Each task owns its own native child webview so switching tasks and coming back
// never reloads the page that task was looking at.
pub const DEFAULT_BROWSER_SESSION: &str = "__draft__";
const MAX_LIVE_BROWSERS: usize = 6;
const BROWSER_LABEL_PREFIX: &str = "side-browser-";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserNavigatedPayload {
  session: String,
  url: String,
  title: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserEnsureResult {
  created: bool,
  url: Option<String>,
}

#[derive(Default)]
struct RegistryInner {
  /// session key -> webview label
  labels: HashMap<String, String>,
  /// live sessions, least recently shown first
  order: Vec<String>,
  /// session whose webview owns the shared `browser-state.json` snapshot
  active: Option<String>,
  next_index: u64,
}

#[derive(Default)]
pub struct BrowserRegistry {
  inner: Mutex<RegistryInner>,
}

fn browser_label(index: u64) -> String {
  format!("{BROWSER_LABEL_PREFIX}{index}")
}

fn is_browser_label(label: &str) -> bool {
  label.starts_with(BROWSER_LABEL_PREFIX)
}

fn session_key(raw: Option<String>) -> String {
  raw
    .map(|value| value.trim().to_owned())
    .filter(|value| !value.is_empty())
    .unwrap_or_else(|| DEFAULT_BROWSER_SESSION.to_owned())
}

fn touch_order(order: &mut Vec<String>, session: &str) {
  order.retain(|item| item != session);
  order.push(session.to_owned());
}

/// Sessions to close before adding `incoming`, oldest first. The visible session
/// is never evicted; an evicted task reloads its page when it is opened again.
fn eviction_targets(
  order: &[String],
  active: Option<&str>,
  incoming: &str,
  limit: usize,
) -> Vec<String> {
  let limit = limit.max(1);
  let mut needed = (order.len() + 1).saturating_sub(limit);
  let mut targets = Vec::new();
  for session in order {
    if needed == 0 {
      break;
    }
    if session == incoming || Some(session.as_str()) == active {
      continue;
    }
    targets.push(session.clone());
    needed -= 1;
  }
  targets
}

fn reportable_url(raw: &str) -> Option<String> {
  let trimmed = raw.trim();
  if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("about:blank") {
    return None;
  }
  Some(trimmed.to_owned())
}

impl BrowserRegistry {
  fn guard(&self) -> MutexGuard<'_, RegistryInner> {
    self
      .inner
      .lock()
      .unwrap_or_else(|poisoned| poisoned.into_inner())
  }

  fn label(&self, session: &str) -> Option<String> {
    self.guard().labels.get(session).cloned()
  }

  fn is_active(&self, session: &str) -> bool {
    self.guard().active.as_deref() == Some(session)
  }

  fn active_label(&self) -> Option<String> {
    let inner = self.guard();
    let active = inner.active.as_deref()?;
    inner.labels.get(active).cloned()
  }

  fn set_active(&self, session: &str) {
    let mut inner = self.guard();
    inner.active = Some(session.to_owned());
    if inner.labels.contains_key(session) {
      touch_order(&mut inner.order, session);
    }
  }

  /// Allocate a label for a session that has no webview yet and report the
  /// labels that must be closed to stay under the live-instance budget.
  fn reserve(&self, session: &str) -> (String, Vec<String>) {
    let mut inner = self.guard();
    let active = inner.active.clone();
    let stale = eviction_targets(&inner.order, active.as_deref(), session, MAX_LIVE_BROWSERS);
    let mut closing = Vec::new();
    for key in &stale {
      if let Some(label) = inner.labels.remove(key) {
        closing.push(label);
      }
      inner.order.retain(|item| item != key);
    }
    inner.next_index += 1;
    let label = browser_label(inner.next_index);
    inner.labels.insert(session.to_owned(), label.clone());
    touch_order(&mut inner.order, session);
    (label, closing)
  }

  fn remove(&self, session: &str) -> Option<String> {
    let mut inner = self.guard();
    inner.order.retain(|item| item != session);
    if inner.active.as_deref() == Some(session) {
      inner.active = None;
    }
    inner.labels.remove(session)
  }

  /// Move a live webview from one session key to another without recreating it.
  /// Returns a label to close when the target already owns a webview.
  fn adopt(&self, from: &str, to: &str) -> Option<String> {
    let mut inner = self.guard();
    let label = inner.labels.remove(from)?;
    inner.order.retain(|item| item != from);
    if inner.active.as_deref() == Some(from) {
      inner.active = Some(to.to_owned());
    }
    if inner.labels.contains_key(to) {
      return Some(label);
    }
    inner.labels.insert(to.to_owned(), label);
    touch_order(&mut inner.order, to);
    None
  }
}

fn map_err(error: impl ToString) -> String {
  error.to_string()
}

fn parse_browser_url(raw: &str) -> Result<Url, String> {
  let trimmed = raw.trim();
  if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("about:blank") {
    return Url::parse("about:blank").map_err(map_err);
  }
  if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
    return Url::parse(trimmed).map_err(map_err);
  }
  Url::parse(&format!("https://{trimmed}")).map_err(map_err)
}

fn main_window(app: &AppHandle) -> Result<tauri::Window, String> {
  app
    .get_window("main")
    .ok_or_else(|| "main window is missing".to_owned())
}

fn registry(app: &AppHandle) -> tauri::State<'_, BrowserRegistry> {
  app.state::<BrowserRegistry>()
}

fn session_browser(app: &AppHandle, session: &str) -> Option<Webview> {
  let label = registry(app).label(session)?;
  app.get_webview(&label)
}

fn close_labels(app: &AppHandle, labels: &[String]) {
  for label in labels {
    if let Some(webview) = app.get_webview(label) {
      let _ = webview.close();
    }
  }
}

fn require_browser(app: &AppHandle, session: &str) -> Result<Webview, String> {
  session_browser(app, session).ok_or_else(|| "side browser is not ready".to_owned())
}

/// Only one task's page may ever be on screen. Sweeping by real webview label
/// instead of registry bookkeeping also clears any orphan that a failed close or
/// a late `show` left floating over the desktop DOM.
fn hide_other_browsers(app: &AppHandle, keep_label: &str) {
  for (label, webview) in app.webviews() {
    if is_browser_label(&label) && label != keep_label {
      let _ = webview.hide();
    }
  }
}

/// Hide everything that is not the task currently in view.
fn hide_inactive_browsers(app: &AppHandle) {
  let keep = registry(app)
    .active_label()
    .unwrap_or_else(|| String::new());
  hide_other_browsers(app, &keep);
}

fn set_bounds(webview: &Webview, x: f64, y: f64, width: f64, height: f64) -> Result<(), String> {
  let width = width.max(1.0);
  let height = height.max(1.0);
  webview
    .set_position(LogicalPosition::new(x, y))
    .map_err(map_err)?;
  webview
    .set_size(LogicalSize::new(width, height))
    .map_err(map_err)?;
  Ok(())
}

fn browser_state_path() -> PathBuf {
  if let Ok(path) = std::env::var("XCODING_BROWSER_STATE_PATH") {
    let trimmed = path.trim();
    if !trimmed.is_empty() {
      return PathBuf::from(trimmed);
    }
  }
  user_config_dir().join("browser-state.json")
}

fn persist_browser_state(available: bool, url: &str, title: &str, visible: bool) {
  let path = browser_state_path();
  if let Some(parent) = path.parent() {
    let _ = std::fs::create_dir_all(parent);
  }
  let updated_at = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|duration| duration.as_millis())
    .unwrap_or(0);
  let payload = json!({
    "available": available,
    "url": url,
    "title": title,
    "visible": visible,
    "updated_at": updated_at,
  });
  let Ok(raw) = serde_json::to_string_pretty(&payload) else {
    return;
  };
  let tmp = path.with_extension("json.tmp");
  if std::fs::write(&tmp, raw).is_err() {
    return;
  }
  if std::fs::rename(&tmp, &path).is_err() {
    let _ = std::fs::copy(&tmp, &path);
    let _ = std::fs::remove_file(&tmp);
  }
}

fn persist_browser_visibility(app: &AppHandle, session: &str, visible: bool) {
  // The snapshot describes what the user can see, so only the visible task writes it.
  if !registry(app).is_active(session) {
    return;
  }
  let Some(webview) = session_browser(app, session) else {
    persist_browser_state(false, "", "", false);
    return;
  };
  let url = webview
    .url()
    .map(|value| value.to_string())
    .unwrap_or_default();
  // Keep the last known title from the snapshot when only visibility changes.
  let title = std::fs::read_to_string(browser_state_path())
    .ok()
    .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
    .and_then(|value| {
      value
        .get("title")
        .and_then(|item| item.as_str())
        .map(str::to_owned)
    })
    .unwrap_or_default();
  persist_browser_state(true, &url, &title, visible);
}

fn emit_navigated(app: &AppHandle, session: &str, url: String, title: String) {
  if registry(app).is_active(session) {
    // Webview has no is_visible(); keep the last visibility bit, default shown.
    let visible = std::fs::read_to_string(browser_state_path())
      .ok()
      .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
      .and_then(|value| value.get("visible").and_then(|item| item.as_bool()))
      .unwrap_or(true);
    persist_browser_state(true, &url, &title, visible);
  }
  let _ = app.emit(
    "browser-navigated",
    BrowserNavigatedPayload {
      session: session.to_owned(),
      url,
      title,
    },
  );
}

fn attach_page_load(
  builder: WebviewBuilder<tauri::Wry>,
  session: String,
) -> WebviewBuilder<tauri::Wry> {
  builder.on_page_load(move |webview, payload| {
    if payload.event() != PageLoadEvent::Finished {
      return;
    }
    let url = payload.url().to_string();
    let app = webview.app_handle().clone();
    let title_url = url.clone();
    let owner = session.clone();
    let fallback_owner = session.clone();
    if webview
      .eval_with_callback(
        "(function(){ try { return document.title || ''; } catch (e) { return ''; } })()",
        move |raw| {
          let title = serde_json::from_str::<String>(&raw)
            .ok()
            .or_else(|| {
              let trimmed = raw.trim().trim_matches('"');
              if trimmed.is_empty() {
                None
              } else {
                Some(trimmed.to_owned())
              }
            })
            .unwrap_or_default();
          emit_navigated(&app, &owner, title_url.clone(), title);
        },
      )
      .is_err()
    {
      emit_navigated(webview.app_handle(), &fallback_owner, url, String::new());
    }
  })
}

fn create_side_browser(
  app: &AppHandle,
  session: &str,
  label: &str,
  x: f64,
  y: f64,
  width: f64,
  height: f64,
  url: &str,
  user_agent: Option<&str>,
) -> Result<(), String> {
  let window = main_window(app)?;
  let parsed = parse_browser_url(url)?;
  let builder = attach_page_load(
    WebviewBuilder::new(label, WebviewUrl::External(parsed)),
    session.to_owned(),
  );
  let builder = match user_agent.map(str::trim).filter(|value| !value.is_empty()) {
    Some(value) => builder.user_agent(value),
    None => builder,
  };
  let webview = window
    .add_child(
      builder,
      LogicalPosition::new(x, y),
      LogicalSize::new(width.max(1.0), height.max(1.0)),
    )
    .map_err(map_err)?;
  let visible = reportable_url(url).is_some();
  if visible {
    hide_other_browsers(app, label);
    let _ = webview.show();
  } else {
    let _ = webview.hide();
  }
  if registry(app).is_active(session) {
    if visible {
      persist_browser_state(true, url, "", true);
    } else {
      persist_browser_state(true, "about:blank", "", false);
    }
  }
  Ok(())
}

fn browser_navigate_inner(webview: &Webview, url: &str) -> Result<(), String> {
  let parsed = parse_browser_url(url)?;
  webview.navigate(parsed).map_err(map_err)
}

fn default_download_dir() -> Result<PathBuf, String> {
  if let Ok(home) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
    let downloads = PathBuf::from(&home).join("Downloads");
    if downloads.is_dir() {
      return Ok(downloads);
    }
    let pictures = PathBuf::from(&home).join("Pictures");
    if pictures.is_dir() {
      return Ok(pictures);
    }
    return Ok(PathBuf::from(home));
  }
  std::env::temp_dir()
    .canonicalize()
    .map_err(map_err)
    .or_else(|_| Ok(std::env::temp_dir()))
}

use std::sync::mpsc;

#[tauri::command]
pub async fn browser_ensure(
  app: AppHandle,
  session: Option<String>,
  x: f64,
  y: f64,
  width: f64,
  height: f64,
  url: Option<String>,
  user_agent: Option<String>,
) -> Result<BrowserEnsureResult, String> {
  let session = session_key(session);
  registry(&app).set_active(&session);
  if let Some(webview) = session_browser(&app, &session) {
    // The task already owns a live page: re-place it and keep it loaded.
    hide_other_browsers(&app, &webview.label().to_owned());
    set_bounds(&webview, x, y, width, height)?;
    let live = webview.url().map(|value| value.to_string()).ok();
    return Ok(BrowserEnsureResult {
      created: false,
      url: live.as_deref().and_then(reportable_url),
    });
  }
  let target = url.unwrap_or_else(|| "about:blank".to_owned());
  let (label, stale) = registry(&app).reserve(&session);
  close_labels(&app, &stale);
  hide_other_browsers(&app, &label);
  create_side_browser(
    &app,
    &session,
    &label,
    x,
    y,
    width,
    height,
    &target,
    user_agent.as_deref(),
  )?;
  Ok(BrowserEnsureResult {
    created: true,
    url: reportable_url(&target),
  })
}

#[tauri::command]
pub async fn browser_set_bounds(
  app: AppHandle,
  session: Option<String>,
  x: f64,
  y: f64,
  width: f64,
  height: f64,
) -> Result<(), String> {
  let Some(webview) = session_browser(&app, &session_key(session)) else {
    return Ok(());
  };
  set_bounds(&webview, x, y, width, height)
}

#[tauri::command]
pub async fn browser_navigate(
  app: AppHandle,
  session: Option<String>,
  url: String,
) -> Result<(), String> {
  let session = session_key(session);
  let webview = require_browser(&app, &session)?;
  browser_navigate_inner(&webview, &url)?;
  // A background task may finish loading at any time; it must not pop to the front.
  if registry(&app).is_active(&session) {
    hide_other_browsers(&app, &webview.label().to_owned());
    let _ = webview.show();
  }
  Ok(())
}

#[tauri::command]
pub async fn browser_reload(app: AppHandle, session: Option<String>) -> Result<(), String> {
  let webview = require_browser(&app, &session_key(session))?;
  webview.reload().map_err(map_err)
}

#[tauri::command]
pub async fn browser_back(app: AppHandle, session: Option<String>) -> Result<(), String> {
  let webview = require_browser(&app, &session_key(session))?;
  webview.eval("history.back()").map_err(map_err)
}

#[tauri::command]
pub async fn browser_forward(app: AppHandle, session: Option<String>) -> Result<(), String> {
  let webview = require_browser(&app, &session_key(session))?;
  webview.eval("history.forward()").map_err(map_err)
}

#[tauri::command]
pub async fn browser_show(app: AppHandle, session: Option<String>) -> Result<(), String> {
  let session = session_key(session);
  // A panel that already switched away can still have an in-flight show; ignore it.
  if !registry(&app).is_active(&session) {
    return Ok(());
  }
  let Some(webview) = session_browser(&app, &session) else {
    return Ok(());
  };
  hide_other_browsers(&app, &webview.label().to_owned());
  webview.show().map_err(map_err)?;
  persist_browser_visibility(&app, &session, true);
  Ok(())
}

#[tauri::command]
pub async fn browser_hide(app: AppHandle, session: Option<String>) -> Result<(), String> {
  let session = session_key(session);
  let Some(webview) = session_browser(&app, &session) else {
    // The label may already be gone (adopted or evicted); make sure nothing lingers.
    hide_inactive_browsers(&app);
    return Ok(());
  };
  webview.hide().map_err(map_err)?;
  persist_browser_visibility(&app, &session, false);
  Ok(())
}

#[tauri::command]
pub async fn browser_close(app: AppHandle, session: Option<String>) -> Result<(), String> {
  let session = session_key(session);
  let was_active = registry(&app).is_active(&session);
  let Some(label) = registry(&app).remove(&session) else {
    return Ok(());
  };
  if let Some(webview) = app.get_webview(&label) {
    webview.close().map_err(map_err)?;
  }
  hide_inactive_browsers(&app);
  if was_active {
    persist_browser_state(false, "", "", false);
  }
  Ok(())
}

/// Carry a draft task's live webview over to the session id it just became,
/// so promoting a draft never reloads the page it already opened.
#[tauri::command]
pub async fn browser_adopt_session(app: AppHandle, from: String, to: String) -> Result<(), String> {
  let from = session_key(Some(from));
  let to = session_key(Some(to));
  if from == to {
    return Ok(());
  }
  if let Some(duplicate) = registry(&app).adopt(&from, &to) {
    close_labels(&app, &[duplicate]);
  }
  hide_inactive_browsers(&app);
  Ok(())
}

#[tauri::command]
pub async fn browser_set_user_agent(
  app: AppHandle,
  session: Option<String>,
  user_agent: Option<String>,
) -> Result<(), String> {
  let session = session_key(session);
  let Some(webview) = session_browser(&app, &session) else {
    return Ok(());
  };
  let url = webview.url().map_err(map_err)?.to_string();
  let position = webview.position().map_err(map_err)?;
  let size = webview.size().map_err(map_err)?;
  webview.close().map_err(map_err)?;
  registry(&app).remove(&session);
  registry(&app).set_active(&session);
  let (label, stale) = registry(&app).reserve(&session);
  close_labels(&app, &stale);
  create_side_browser(
    &app,
    &session,
    &label,
    f64::from(position.x),
    f64::from(position.y),
    f64::from(size.width),
    f64::from(size.height),
    &url,
    user_agent.as_deref(),
  )
}

#[tauri::command]
pub async fn browser_set_zoom(
  app: AppHandle,
  session: Option<String>,
  scale_factor: f64,
) -> Result<(), String> {
  let webview = require_browser(&app, &session_key(session))?;
  let scale = scale_factor.clamp(0.5, 3.0);
  webview.set_zoom(scale).map_err(map_err)
}

#[tauri::command]
pub async fn browser_print(app: AppHandle, session: Option<String>) -> Result<(), String> {
  let webview = require_browser(&app, &session_key(session))?;
  webview.print().map_err(map_err)
}

#[tauri::command]
pub async fn browser_clear_data(app: AppHandle, session: Option<String>) -> Result<(), String> {
  let webview = require_browser(&app, &session_key(session))?;
  webview.clear_all_browsing_data().map_err(map_err)
}

#[tauri::command]
pub async fn browser_current_url(
  app: AppHandle,
  session: Option<String>,
) -> Result<Option<String>, String> {
  let Some(webview) = session_browser(&app, &session_key(session)) else {
    return Ok(None);
  };
  let url = webview.url().map_err(map_err)?;
  Ok(Some(url.to_string()))
}

#[tauri::command]
pub async fn browser_eval(
  app: AppHandle,
  session: Option<String>,
  script: String,
) -> Result<(), String> {
  let webview = require_browser(&app, &session_key(session))?;
  webview.eval(script).map_err(map_err)
}

#[tauri::command]
pub async fn browser_find(
  app: AppHandle,
  session: Option<String>,
  query: String,
  forward: bool,
  match_case: bool,
) -> Result<(), String> {
  let webview = require_browser(&app, &session_key(session))?;
  let q = serde_json::to_string(&query).map_err(map_err)?;
  let backwards = if forward { "false" } else { "true" };
  let case_sensitive = if match_case { "true" } else { "false" };
  let script = format!(
    "(function(){{ try {{ window.find({q}, {case_sensitive}, {backwards}, true, false, true, false); }} catch (e) {{}} }})();"
  );
  webview.eval(script).map_err(map_err)
}

#[tauri::command]
pub async fn browser_download_dir() -> Result<String, String> {
  Ok(default_download_dir()?.display().to_string())
}

#[tauri::command]
pub async fn browser_save_snapshot(
  app: AppHandle,
  session: Option<String>,
) -> Result<String, String> {
  let webview = require_browser(&app, &session_key(session))?;
  let dir = default_download_dir()?;
  std::fs::create_dir_all(&dir).map_err(map_err)?;
  let stamp = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|d| d.as_millis())
    .unwrap_or(0);
  let pdf = dir.join(format!("xcoding-browser-{stamp}.pdf"));
  let (tx, rx) = mpsc::channel();
  webview
    .print_to_pdf(pdf.clone(), Box::new(move |ok| {
      let _ = tx.send(ok);
    }))
    .map_err(|error| format!("failed to start PDF export: {error}"))?;
  let ok = rx
    .recv_timeout(std::time::Duration::from_secs(30))
    .map_err(|_| "PDF export timed out".to_owned())?;
  if !ok {
    return Err("PDF export failed".to_owned());
  }
  if !pdf.is_file() {
    return Err("PDF file was not created".to_owned());
  }
  Ok(pdf.display().to_string())
}

#[cfg(test)]
mod tests {
  use super::{
    browser_label, eviction_targets, is_browser_label, reportable_url, session_key,
    BrowserRegistry, DEFAULT_BROWSER_SESSION, MAX_LIVE_BROWSERS,
  };

  #[test]
  fn session_key_falls_back_to_the_draft_bucket() {
    assert_eq!(session_key(None), DEFAULT_BROWSER_SESSION);
    assert_eq!(session_key(Some("   ".to_owned())), DEFAULT_BROWSER_SESSION);
    assert_eq!(session_key(Some(" abc ".to_owned())), "abc");
  }

  #[test]
  fn labels_stay_unique_per_session() {
    let registry = BrowserRegistry::default();
    registry.set_active("task-a");
    let (first, evicted) = registry.reserve("task-a");
    assert!(evicted.is_empty());
    registry.set_active("task-b");
    let (second, _) = registry.reserve("task-b");
    assert_ne!(first, second);
    assert_eq!(registry.label("task-a").as_deref(), Some(first.as_str()));
    assert_eq!(registry.label("task-b").as_deref(), Some(second.as_str()));
    assert_eq!(browser_label(3), "side-browser-3");
  }

  #[test]
  fn only_the_visible_session_owns_the_shared_snapshot() {
    let registry = BrowserRegistry::default();
    registry.set_active("task-a");
    registry.reserve("task-a");
    registry.set_active("task-b");
    registry.reserve("task-b");
    assert!(!registry.is_active("task-a"));
    assert!(registry.is_active("task-b"));
  }

  #[test]
  fn eviction_skips_the_visible_and_incoming_sessions() {
    let order = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
    assert!(eviction_targets(&order, Some("a"), "d", 4).is_empty());
    assert_eq!(eviction_targets(&order, Some("a"), "d", 3), vec!["b".to_owned()]);
    assert_eq!(
      eviction_targets(&order, Some("a"), "d", 2),
      vec!["b".to_owned(), "c".to_owned()],
    );
    assert_eq!(eviction_targets(&order, Some("b"), "c", 2), vec!["a".to_owned()]);
  }

  #[test]
  fn reserve_evicts_the_oldest_background_session_at_the_limit() {
    let registry = BrowserRegistry::default();
    let mut labels = Vec::new();
    for index in 0..MAX_LIVE_BROWSERS {
      let session = format!("task-{index}");
      registry.set_active(&session);
      let (label, evicted) = registry.reserve(&session);
      assert!(evicted.is_empty(), "no eviction below the limit");
      labels.push(label);
    }
    registry.set_active("task-new");
    let (_, evicted) = registry.reserve("task-new");
    assert_eq!(evicted, vec![labels[0].clone()]);
    assert!(registry.label("task-0").is_none());
    assert!(registry.label("task-1").is_some());
  }

  #[test]
  fn adopt_moves_a_live_webview_to_the_created_session() {
    let registry = BrowserRegistry::default();
    registry.set_active(DEFAULT_BROWSER_SESSION);
    let (label, _) = registry.reserve(DEFAULT_BROWSER_SESSION);
    assert_eq!(registry.adopt(DEFAULT_BROWSER_SESSION, "session-1"), None);
    assert_eq!(registry.label("session-1").as_deref(), Some(label.as_str()));
    assert!(registry.label(DEFAULT_BROWSER_SESSION).is_none());
    assert!(registry.is_active("session-1"));
  }

  #[test]
  fn adopt_returns_the_duplicate_when_the_target_already_has_one() {
    let registry = BrowserRegistry::default();
    registry.set_active("session-1");
    registry.reserve("session-1");
    registry.set_active(DEFAULT_BROWSER_SESSION);
    let (draft, _) = registry.reserve(DEFAULT_BROWSER_SESSION);
    assert_eq!(
      registry.adopt(DEFAULT_BROWSER_SESSION, "session-1"),
      Some(draft),
    );
    assert!(registry.label(DEFAULT_BROWSER_SESSION).is_none());
    assert!(registry.label("session-1").is_some());
  }

  #[test]
  fn remove_drops_the_mapping_and_clears_the_visible_session() {
    let registry = BrowserRegistry::default();
    registry.set_active("task-a");
    let (label, _) = registry.reserve("task-a");
    assert_eq!(registry.remove("task-a").as_deref(), Some(label.as_str()));
    assert!(registry.label("task-a").is_none());
    assert!(!registry.is_active("task-a"));
    assert!(registry.remove("task-a").is_none());
  }

  #[test]
  fn blank_pages_are_not_reported_as_a_live_url() {
    assert_eq!(reportable_url("  "), None);
    assert_eq!(reportable_url("about:blank"), None);
    assert_eq!(reportable_url("ABOUT:BLANK"), None);
    assert_eq!(
      reportable_url(" https://example.com/a "),
      Some("https://example.com/a".to_owned()),
    );
  }

  #[test]
  fn only_side_browser_labels_are_swept() {
    assert!(is_browser_label(&browser_label(1)));
    assert!(is_browser_label("side-browser-42"));
    assert!(!is_browser_label("main"));
    assert!(!is_browser_label(""));
  }

  #[test]
  fn active_label_points_at_the_visible_task() {
    let registry = BrowserRegistry::default();
    assert_eq!(registry.active_label(), None);
    registry.set_active("task-a");
    // Active but not created yet: nothing may stay on screen.
    assert_eq!(registry.active_label(), None);
    let (label_a, _) = registry.reserve("task-a");
    assert_eq!(registry.active_label(), Some(label_a));
    registry.set_active("task-b");
    let (label_b, _) = registry.reserve("task-b");
    assert_eq!(registry.active_label(), Some(label_b));
    registry.remove("task-b");
    assert_eq!(registry.active_label(), None);
  }
}
