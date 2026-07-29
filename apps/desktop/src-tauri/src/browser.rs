use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{
  AppHandle, Emitter, Manager, LogicalPosition, LogicalSize, Url, Webview, WebviewUrl,
  webview::{PageLoadEvent, WebviewBuilder},
};

const SIDE_BROWSER_LABEL: &str = "side-browser";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserNavigatedPayload {
  url: String,
  title: String,
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

fn side_browser(app: &AppHandle) -> Option<Webview> {
  app.get_webview(SIDE_BROWSER_LABEL)
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

fn emit_navigated(app: &AppHandle, url: String, title: String) {
  let _ = app.emit(
    "browser-navigated",
    BrowserNavigatedPayload { url, title },
  );
}

fn attach_page_load(builder: WebviewBuilder<tauri::Wry>) -> WebviewBuilder<tauri::Wry> {
  builder.on_page_load(|webview, payload| {
    if payload.event() != PageLoadEvent::Finished {
      return;
    }
    let url = payload.url().to_string();
    let app = webview.app_handle().clone();
    let title_url = url.clone();
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
          emit_navigated(&app, title_url.clone(), title);
        },
      )
      .is_err()
    {
      emit_navigated(webview.app_handle(), url, String::new());
    }
  })
}

fn create_side_browser(
  app: &AppHandle,
  x: f64,
  y: f64,
  width: f64,
  height: f64,
  url: &str,
  user_agent: Option<&str>,
) -> Result<(), String> {
  let window = main_window(app)?;
  let parsed = parse_browser_url(url)?;
  let builder = attach_page_load(WebviewBuilder::new(
    SIDE_BROWSER_LABEL,
    WebviewUrl::External(parsed),
  ));
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
  if url.trim().is_empty() || url.trim().eq_ignore_ascii_case("about:blank") {
    let _ = webview.hide();
  } else {
    let _ = webview.show();
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

fn capture_screen_region(x: i32, y: i32, width: u32, height: u32, path: &Path) -> Result<(), String> {
  #[cfg(windows)]
  {
    let width = width.max(1);
    let height = height.max(1);
    let path_str = path.to_string_lossy().replace('\'', "''");
    let script = format!(
      "Add-Type -AssemblyName System.Drawing; $b = New-Object System.Drawing.Bitmap({width},{height}); $g = [System.Drawing.Graphics]::FromImage($b); $g.CopyFromScreen({x},{y},0,0,(New-Object System.Drawing.Size({width},{height}))); $b.Save('{path_str}', [System.Drawing.Imaging.ImageFormat]::Png); $g.Dispose(); $b.Dispose();"
    );
    let output = Command::new("powershell")
      .args([
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &script,
      ])
      .output()
      .map_err(|error| format!("failed to capture screenshot: {error}"))?;
    if !output.status.success() {
      let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
      let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
      return Err(if !stderr.is_empty() {
        stderr
      } else if !stdout.is_empty() {
        stdout
      } else {
        "screenshot capture failed".to_owned()
      });
    }
    if !path.is_file() {
      return Err("screenshot file was not created".to_owned());
    }
    Ok(())
  }
  #[cfg(not(windows))]
  {
    let _ = (x, y, width, height, path);
    Err("screenshot is only supported on Windows".to_owned())
  }
}

#[tauri::command]
pub async fn browser_ensure(
  app: AppHandle,
  x: f64,
  y: f64,
  width: f64,
  height: f64,
  url: Option<String>,
  user_agent: Option<String>,
) -> Result<(), String> {
  let target = url.unwrap_or_else(|| "about:blank".to_owned());
  if let Some(webview) = side_browser(&app) {
    set_bounds(&webview, x, y, width, height)?;
    if !target.trim().is_empty() && !target.trim().eq_ignore_ascii_case("about:blank") {
      browser_navigate_inner(&webview, &target)?;
      let _ = webview.show();
    }
    return Ok(());
  }
  create_side_browser(&app, x, y, width, height, &target, user_agent.as_deref())
}

#[tauri::command]
pub async fn browser_set_bounds(
  app: AppHandle,
  x: f64,
  y: f64,
  width: f64,
  height: f64,
) -> Result<(), String> {
  let Some(webview) = side_browser(&app) else {
    return Ok(());
  };
  set_bounds(&webview, x, y, width, height)
}

#[tauri::command]
pub async fn browser_navigate(app: AppHandle, url: String) -> Result<(), String> {
  let webview = side_browser(&app).ok_or_else(|| "side browser is not ready".to_owned())?;
  browser_navigate_inner(&webview, &url)?;
  let _ = webview.show();
  Ok(())
}

#[tauri::command]
pub async fn browser_reload(app: AppHandle) -> Result<(), String> {
  let webview = side_browser(&app).ok_or_else(|| "side browser is not ready".to_owned())?;
  webview.reload().map_err(map_err)
}

#[tauri::command]
pub async fn browser_back(app: AppHandle) -> Result<(), String> {
  let webview = side_browser(&app).ok_or_else(|| "side browser is not ready".to_owned())?;
  webview.eval("history.back()").map_err(map_err)
}

#[tauri::command]
pub async fn browser_forward(app: AppHandle) -> Result<(), String> {
  let webview = side_browser(&app).ok_or_else(|| "side browser is not ready".to_owned())?;
  webview.eval("history.forward()").map_err(map_err)
}

#[tauri::command]
pub async fn browser_show(app: AppHandle) -> Result<(), String> {
  let Some(webview) = side_browser(&app) else {
    return Ok(());
  };
  webview.show().map_err(map_err)
}

#[tauri::command]
pub async fn browser_hide(app: AppHandle) -> Result<(), String> {
  let Some(webview) = side_browser(&app) else {
    return Ok(());
  };
  webview.hide().map_err(map_err)
}

#[tauri::command]
pub async fn browser_close(app: AppHandle) -> Result<(), String> {
  let Some(webview) = side_browser(&app) else {
    return Ok(());
  };
  webview.close().map_err(map_err)
}

#[tauri::command]
pub async fn browser_set_user_agent(app: AppHandle, user_agent: Option<String>) -> Result<(), String> {
  let Some(webview) = side_browser(&app) else {
    return Ok(());
  };
  let url = webview.url().map_err(map_err)?.to_string();
  let position = webview.position().map_err(map_err)?;
  let size = webview.size().map_err(map_err)?;
  webview.close().map_err(map_err)?;
  create_side_browser(
    &app,
    f64::from(position.x),
    f64::from(position.y),
    f64::from(size.width),
    f64::from(size.height),
    &url,
    user_agent.as_deref(),
  )
}

#[tauri::command]
pub async fn browser_set_zoom(app: AppHandle, scale_factor: f64) -> Result<(), String> {
  let webview = side_browser(&app).ok_or_else(|| "side browser is not ready".to_owned())?;
  let scale = scale_factor.clamp(0.5, 3.0);
  webview.set_zoom(scale).map_err(map_err)
}

#[tauri::command]
pub async fn browser_print(app: AppHandle) -> Result<(), String> {
  let webview = side_browser(&app).ok_or_else(|| "side browser is not ready".to_owned())?;
  webview.print().map_err(map_err)
}

#[tauri::command]
pub async fn browser_clear_data(app: AppHandle) -> Result<(), String> {
  let webview = side_browser(&app).ok_or_else(|| "side browser is not ready".to_owned())?;
  webview.clear_all_browsing_data().map_err(map_err)
}

#[tauri::command]
pub async fn browser_current_url(app: AppHandle) -> Result<Option<String>, String> {
  let Some(webview) = side_browser(&app) else {
    return Ok(None);
  };
  let url = webview.url().map_err(map_err)?;
  Ok(Some(url.to_string()))
}

#[tauri::command]
pub async fn browser_eval(app: AppHandle, script: String) -> Result<(), String> {
  let webview = side_browser(&app).ok_or_else(|| "side browser is not ready".to_owned())?;
  webview.eval(script).map_err(map_err)
}

#[tauri::command]
pub async fn browser_find(
  app: AppHandle,
  query: String,
  forward: bool,
  match_case: bool,
) -> Result<(), String> {
  let webview = side_browser(&app).ok_or_else(|| "side browser is not ready".to_owned())?;
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
pub async fn browser_screenshot(app: AppHandle) -> Result<String, String> {
  let webview = side_browser(&app).ok_or_else(|| "side browser is not ready".to_owned())?;
  let pos = webview.position().map_err(map_err)?;
  let size = webview.size().map_err(map_err)?;
  let width = size.width.max(1);
  let height = size.height.max(1);
  let dir = default_download_dir()?;
  std::fs::create_dir_all(&dir).map_err(map_err)?;
  let stamp = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|d| d.as_millis())
    .unwrap_or(0);
  let path = dir.join(format!("xcoding-browser-{stamp}.png"));
  capture_screen_region(pos.x, pos.y, width, height, &path)?;
  Ok(path.display().to_string())
}
