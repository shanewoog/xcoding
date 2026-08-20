import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

async function main() {
  const [storeSource, commandSource, registrationSource, apiSource, panelSource, i18nSource, cssSource] = await Promise.all([
    readFile(resolve(repositoryRoot, "apps/desktop/src-tauri/src/passwords.rs"), "utf8"),
    readFile(resolve(repositoryRoot, "apps/desktop/src-tauri/src/browser.rs"), "utf8"),
    readFile(resolve(repositoryRoot, "apps/desktop/src-tauri/src/main.rs"), "utf8"),
    readFile(resolve(repositoryRoot, "apps/desktop/src/workspaceApi.ts"), "utf8"),
    readFile(resolve(repositoryRoot, "apps/desktop/src/panels.tsx"), "utf8"),
    readFile(resolve(repositoryRoot, "apps/desktop/src/i18n.ts"), "utf8"),
    readFile(resolve(repositoryRoot, "apps/desktop/src/styles.css"), "utf8"),
  ]);

  // Secrets are sealed by the OS, never written or handed out in clear text.
  assert.ok(storeSource.includes("CryptProtectData"), "passwords must be sealed with DPAPI before they are stored");
  assert.ok(storeSource.includes("CryptUnprotectData"), "revealing a password must go through DPAPI");
  assert.ok(
    storeSource.includes("CRYPTPROTECT_UI_FORBIDDEN"),
    "encryption must never pop a UI prompt from a background command",
  );
  const entryStruct = storeSource.match(/pub struct PasswordEntry \{([\s\S]*?)\n\}/);
  assert.ok(entryStruct, "the UI-facing password entry struct must exist");
  assert.ok(!/\bpassword\b|\bsecret\b/.test(entryStruct[1]), "the list payload must not carry the secret");

  // Writes stay atomic so a crash cannot truncate saved credentials.
  assert.ok(storeSource.includes('with_extension("json.tmp")'), "the store must be written through a temporary file");

  // One credential per origin, and ambiguity is reported instead of guessed.
  assert.ok(storeSource.includes("origin().ascii_serialization()"), "sites must be keyed by origin, not by full URL");
  assert.ok(
    storeSource.includes("several accounts are saved for this site"),
    "autofill must refuse to guess when a site has several accounts",
  );

  // The six commands exist and are all registered on the Tauri handler.
  for (const command of [
    "browser_passwords_list",
    "browser_password_save",
    "browser_password_delete",
    "browser_password_reveal",
    "browser_password_capture",
    "browser_password_fill",
  ]) {
    assert.ok(commandSource.includes(`pub async fn ${command}(`), `${command} must be implemented`);
    assert.ok(registrationSource.includes(`browser::${command}`), `${command} must be registered as a command`);
  }
  assert.ok(registrationSource.includes("mod passwords;"), "the password store module must be part of the binary");

  // Capture cannot hang the UI if the page never answers the eval callback.
  assert.ok(commandSource.includes("recv_timeout"), "capturing a password must time out instead of hanging");

  // Frontend wiring: every command has a wrapper and the panel uses them.
  for (const wrapper of [
    "browserPasswordsList",
    "browserPasswordSave",
    "browserPasswordDelete",
    "browserPasswordReveal",
    "browserPasswordCapture",
    "browserPasswordFill",
  ]) {
    assert.ok(apiSource.includes(`export async function ${wrapper}(`), `${wrapper} must be exposed to the UI`);
    assert.ok(panelSource.includes(wrapper), `${wrapper} must be used by the browser panel`);
  }
  assert.ok(
    panelSource.includes('t(locale, "browser.passwordCaptureMenu")') &&
      panelSource.includes('t(locale, "browser.passwordFillMenu")'),
    "capture and fill must be reachable from the browser menu",
  );

  // The disabled placeholder and the contact-info block are gone for good.
  assert.ok(!panelSource.includes("browser.contactInfo"), "the contact info rows must be removed");
  assert.ok(!i18nSource.includes("browser.contactInfo"), "the contact info strings must be removed from both locales");
  assert.ok(
    !i18nSource.includes("Saved passwords are not stored in XCoding yet") &&
      !i18nSource.includes("XCoding 暂不保存网站密码"),
    "the password manager help text must no longer claim passwords are unsupported",
  );

  // Both locales stay in sync for the new keys.
  const enStart = i18nSource.indexOf("const en = {");
  const zhStart = i18nSource.indexOf("const zhCN");
  const zhEnd = i18nSource.indexOf("\n};", zhStart);
  assert.ok(enStart >= 0 && zhStart > enStart && zhEnd > zhStart, "both locale tables must be present");
  const localeBlocks = [i18nSource.slice(enStart, zhStart), i18nSource.slice(zhStart, zhEnd)];
  for (const key of [
    "browser.passwordCapture",
    "browser.passwordCaptureMenu",
    "browser.passwordFillMenu",
    "browser.passwordSite",
    "browser.passwordUsername",
    "browser.passwordValue",
    "browser.passwordAdd",
    "browser.passwordEmpty",
    "browser.passwordNoUsername",
    "browser.passwordShow",
    "browser.passwordHide",
    "browser.passwordRemove",
    "browser.passwordSaved",
    "browser.passwordCaptured",
    "browser.passwordNoForm",
    "browser.passwordFilled",
    "browser.passwordNoMatch",
  ]) {
    for (const block of localeBlocks) {
      assert.ok(block.includes(`"${key}"`), `${key} must be localized in every locale table`);
    }
  }

  // Styling stays on theme tokens so the light theme keeps working.
  const passwordCss = cssSource.match(/\.browser-password-form input \{([\s\S]*?)\n\}/);
  assert.ok(passwordCss, "the credential form inputs must be styled");
  assert.ok(!/#[0-9a-fA-F]{3,8}\b/.test(passwordCss[1]), "the credential form must not hardcode colors");
  assert.ok(cssSource.includes(".browser-password-actions {"), "the per-entry action row must be styled");

  console.log("Desktop browser password checks passed.");
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
