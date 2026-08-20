//! Website credentials for the built-in browser.
//!
//! Secrets never touch the store in clear text: every password is sealed with
//! DPAPI under the current Windows user before it is written to disk, and the
//! plaintext only leaves this module for an explicit reveal or an autofill
//! script that goes straight into the page.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::Url;
use xcoding_providers::user_config_dir;

/// Entry as the UI sees it. There is deliberately no password field here.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PasswordEntry {
    pub id: String,
    pub origin: String,
    pub username: String,
    pub updated_at: u128,
}

/// What a capture from the live page reports back, again without the secret.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturedPassword {
    pub origin: String,
    pub username: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct StoredEntry {
    id: String,
    origin: String,
    username: String,
    /// DPAPI blob, hex encoded so the file stays plain JSON.
    secret: String,
    updated_at: u128,
}

#[derive(Default, Serialize, Deserialize)]
struct PasswordStore {
    #[serde(default)]
    entries: Vec<StoredEntry>,
}

/// Payload the capture script extracts from the page.
#[derive(Deserialize)]
struct CapturedForm {
    #[serde(default)]
    origin: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
}

pub fn store_path() -> PathBuf {
    if let Ok(path) = std::env::var("XCODING_BROWSER_PASSWORDS_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    user_config_dir().join("browser-passwords.json")
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0)
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap_or('0'));
    }
    out
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    let raw = text.as_bytes();
    let mut out = Vec::with_capacity(raw.len() / 2);
    for pair in raw.chunks(2) {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        out.push(((high << 4) | low) as u8);
    }
    Some(out)
}

/// Scheme + host + port, so `https://a.test/login?x=1` and `https://a.test/app`
/// share one credential the way a browser password manager does.
pub fn origin_of(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let parsed = Url::parse(trimmed).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let origin = parsed.origin().ascii_serialization();
    if origin.eq_ignore_ascii_case("null") {
        return None;
    }
    Some(origin)
}

fn read_store(path: &Path) -> PasswordStore {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<PasswordStore>(&raw).ok())
        .unwrap_or_default()
}

fn write_store(path: &Path, store: &PasswordStore) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let raw = serde_json::to_string_pretty(store).map_err(|error| error.to_string())?;
    // Same atomic swap the browser snapshot uses: a crash mid-write must not
    // truncate the credentials the user already saved.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, raw).map_err(|error| error.to_string())?;
    if std::fs::rename(&tmp, path).is_err() {
        std::fs::copy(&tmp, path).map_err(|error| error.to_string())?;
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(())
}

fn view(entry: &StoredEntry) -> PasswordEntry {
    PasswordEntry {
        id: entry.id.clone(),
        origin: entry.origin.clone(),
        username: entry.username.clone(),
        updated_at: entry.updated_at,
    }
}

pub fn list(path: &Path) -> Vec<PasswordEntry> {
    let mut entries: Vec<PasswordEntry> = read_store(path).entries.iter().map(view).collect();
    entries.sort_by(|left, right| {
        left.origin
            .cmp(&right.origin)
            .then_with(|| left.username.cmp(&right.username))
    });
    entries
}

pub fn save(
    path: &Path,
    origin: &str,
    username: &str,
    password: &str,
) -> Result<PasswordEntry, String> {
    let origin = origin_of(origin).ok_or_else(|| "enter a http(s) site address".to_owned())?;
    let username = username.trim().to_owned();
    if password.is_empty() {
        return Err("enter a password".to_owned());
    }
    let secret = hex_encode(&protect(password)?);
    let mut store = read_store(path);
    let updated_at = now_millis();
    let existing = store
        .entries
        .iter_mut()
        .find(|entry| entry.origin == origin && entry.username == username);
    let entry = match existing {
        Some(entry) => {
            entry.secret = secret;
            entry.updated_at = updated_at;
            view(entry)
        }
        None => {
            let entry = StoredEntry {
                id: uuid::Uuid::new_v4().to_string(),
                origin,
                username,
                secret,
                updated_at,
            };
            let created = view(&entry);
            store.entries.push(entry);
            created
        }
    };
    write_store(path, &store)?;
    Ok(entry)
}

pub fn delete(path: &Path, id: &str) -> Result<bool, String> {
    let mut store = read_store(path);
    let before = store.entries.len();
    store.entries.retain(|entry| entry.id != id);
    if store.entries.len() == before {
        return Ok(false);
    }
    write_store(path, &store)?;
    Ok(true)
}

pub fn reveal(path: &Path, id: &str) -> Result<String, String> {
    let store = read_store(path);
    let entry = store
        .entries
        .iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| "saved password is missing".to_owned())?;
    let blob = hex_decode(&entry.secret).ok_or_else(|| "saved password is damaged".to_owned())?;
    unprotect(&blob)
}

/// Credential to autofill on `url`, if exactly one origin match is stored.
pub fn credential_for(path: &Path, url: &str) -> Result<Option<(String, String)>, String> {
    let Some(origin) = origin_of(url) else {
        return Ok(None);
    };
    let store = read_store(path);
    let mut matches = store.entries.iter().filter(|entry| entry.origin == origin);
    let Some(entry) = matches.next() else {
        return Ok(None);
    };
    // Several accounts on one site: let the user pick instead of guessing.
    if matches.next().is_some() {
        return Err("several accounts are saved for this site".to_owned());
    }
    let blob = hex_decode(&entry.secret).ok_or_else(|| "saved password is damaged".to_owned())?;
    Ok(Some((entry.username.clone(), unprotect(&blob)?)))
}

/// Reads the sign-in form the page currently shows. Returns an empty string when
/// there is nothing to save, so the caller can report "no form found".
pub fn capture_script() -> &'static str {
    r#"(function(){
  try {
    var fields = Array.prototype.slice.call(document.querySelectorAll('input[type=password]'));
    var target = null;
    for (var i = 0; i < fields.length; i += 1) {
      if (fields[i].value) { target = fields[i]; break; }
    }
    if (!target || !target.value) return '';
    var scope = target.form || document;
    var text = scope.querySelectorAll('input[type=text], input[type=email], input[type=tel], input:not([type])');
    var username = '';
    for (var j = 0; j < text.length; j += 1) {
      if (text[j].value) { username = text[j].value; break; }
    }
    return JSON.stringify({ origin: location.origin, username: username, password: target.value });
  } catch (error) {
    return '';
  }
})()"#
}

/// Writes a stored credential into the page's sign-in form and fires the events
/// frameworks listen to, otherwise React-controlled inputs snap back.
pub fn fill_script(username: &str, password: &str) -> Result<String, String> {
    let username = serde_json::to_string(username).map_err(|error| error.to_string())?;
    let password = serde_json::to_string(password).map_err(|error| error.to_string())?;
    Ok(format!(
        r#"(function(){{
  try {{
    var pw = document.querySelector('input[type=password]');
    if (!pw) return;
    var scope = pw.form || document;
    var text = scope.querySelectorAll('input[type=text], input[type=email], input[type=tel], input:not([type])');
    var user = text.length ? text[0] : null;
    var apply = function(field, value) {{
      if (!field) return;
      field.focus();
      field.value = value;
      field.dispatchEvent(new Event('input', {{ bubbles: true }}));
      field.dispatchEvent(new Event('change', {{ bubbles: true }}));
    }};
    apply(user, {username});
    apply(pw, {password});
    pw.blur();
  }} catch (error) {{}}
}})();"#
    ))
}

/// Parses whatever `eval_with_callback` handed back. The result arrives as a
/// JSON-encoded value, and the script itself returns a JSON string, so the
/// payload is double encoded.
pub fn parse_capture(raw: &str) -> Option<(CapturedPassword, String)> {
    let value = serde_json::from_str::<serde_json::Value>(raw).ok()?;
    let inner = match value {
        serde_json::Value::String(text) => text,
        other => other.to_string(),
    };
    if inner.trim().is_empty() {
        return None;
    }
    let form = serde_json::from_str::<CapturedForm>(&inner).ok()?;
    if form.password.is_empty() {
        return None;
    }
    let origin = origin_of(&form.origin)?;
    Some((
        CapturedPassword {
            origin,
            username: form.username,
        },
        form.password,
    ))
}

#[cfg(windows)]
fn protect(plain: &str) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData,
    };

    let mut bytes = plain.as_bytes().to_vec();
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let ok = unsafe {
        CryptProtectData(
            &input,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err("Windows refused to encrypt the password".to_owned());
    }
    let sealed =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
    unsafe {
        LocalFree(output.pbData as *mut core::ffi::c_void);
    }
    Ok(sealed)
}

#[cfg(windows)]
fn unprotect(sealed: &[u8]) -> Result<String, String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptUnprotectData,
    };

    if sealed.is_empty() {
        return Err("saved password is damaged".to_owned());
    }
    let mut bytes = sealed.to_vec();
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err("Windows refused to decrypt the password".to_owned());
    }
    let plain =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) }.to_vec();
    unsafe {
        LocalFree(output.pbData as *mut core::ffi::c_void);
    }
    String::from_utf8(plain).map_err(|_| "saved password is damaged".to_owned())
}

#[cfg(not(windows))]
fn protect(_plain: &str) -> Result<Vec<u8>, String> {
    Err("saving passwords needs Windows DPAPI".to_owned())
}

#[cfg(not(windows))]
fn unprotect(_sealed: &[u8]) -> Result<String, String> {
    Err("saving passwords needs Windows DPAPI".to_owned())
}

#[cfg(test)]
mod tests {
    use super::{
        credential_for, delete, fill_script, hex_decode, hex_encode, list, origin_of,
        parse_capture, reveal, save,
    };
    use std::path::PathBuf;

    fn temp_store(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("xcoding-passwords-{name}.json"));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn hex_round_trips_and_rejects_damaged_text() {
        assert_eq!(hex_encode(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
        assert_eq!(hex_decode("000fa5ff"), Some(vec![0x00, 0x0f, 0xa5, 0xff]));
        assert_eq!(hex_decode("abc"), None);
        assert_eq!(hex_decode("zz"), None);
    }

    #[test]
    fn origins_ignore_path_and_reject_non_web_urls() {
        assert_eq!(
            origin_of("https://Example.com/login?next=1").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            origin_of("http://localhost:5173/app").as_deref(),
            Some("http://localhost:5173")
        );
        assert_eq!(origin_of("about:blank"), None);
        assert_eq!(origin_of("file:///C:/tmp/index.html"), None);
        assert_eq!(origin_of("   "), None);
    }

    #[test]
    fn capture_payload_is_parsed_from_the_double_encoded_result() {
        let raw = serde_json::to_string(
            &r#"{"origin":"https://example.com","username":"ada","password":"hunter2"}"#,
        )
        .unwrap();
        let (captured, password) = parse_capture(&raw).expect("payload");
        assert_eq!(captured.origin, "https://example.com");
        assert_eq!(captured.username, "ada");
        assert_eq!(password, "hunter2");
        assert!(parse_capture("\"\"").is_none());
        assert!(parse_capture("null").is_none());
    }

    #[test]
    fn fill_script_escapes_the_values_it_injects() {
        let script = fill_script("ada\"; alert(1); //", "p'a\\ss").unwrap();
        assert!(script.contains(r#"apply(user, "ada\"; alert(1); //");"#));
        assert!(script.contains(r#"apply(pw, "p'a\\ss");"#));
    }

    #[cfg(windows)]
    #[test]
    fn saving_hides_the_secret_on_disk_and_still_reads_it_back() {
        let path = temp_store("roundtrip");
        let entry = save(&path, "https://example.com/login", " ada ", "hunter2").unwrap();
        assert_eq!(entry.origin, "https://example.com");
        assert_eq!(entry.username, "ada");

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains("hunter2"),
            "plaintext must never reach the store"
        );

        assert_eq!(reveal(&path, &entry.id).unwrap(), "hunter2");
        assert_eq!(
            credential_for(&path, "https://example.com/account").unwrap(),
            Some(("ada".to_owned(), "hunter2".to_owned()))
        );
        assert_eq!(credential_for(&path, "https://other.test/").unwrap(), None);

        // Same origin and account updates in place instead of stacking.
        save(&path, "https://example.com/", "ada", "hunter3").unwrap();
        assert_eq!(list(&path).len(), 1);
        assert_eq!(reveal(&path, &entry.id).unwrap(), "hunter3");

        assert!(delete(&path, &entry.id).unwrap());
        assert!(!delete(&path, &entry.id).unwrap());
        assert!(list(&path).is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(windows)]
    #[test]
    fn several_accounts_on_one_site_are_not_guessed() {
        let path = temp_store("ambiguous");
        save(&path, "https://example.com", "ada", "one").unwrap();
        save(&path, "https://example.com", "bob", "two").unwrap();
        assert_eq!(list(&path).len(), 2);
        assert!(credential_for(&path, "https://example.com/login").is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn empty_passwords_and_non_web_origins_are_refused() {
        let path = temp_store("invalid");
        assert!(save(&path, "https://example.com", "ada", "").is_err());
        assert!(save(&path, "about:blank", "ada", "secret").is_err());
        assert!(!path.exists());
    }
}
