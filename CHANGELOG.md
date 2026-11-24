# Changelog

## Unreleased

- Account cards no longer cut off their shortcut and remove buttons when the window is wide enough
  for three or more columns: narrow cards show "Switch" and "Open" instead of the full button
  text (screen readers still hear it in full), and a long plan name is shortened with an ellipsis.

## 1.1.0

- The app keeps running in the tray when its window is closed. The tray menu lists every account
  with its 5-hour and weekly quota; picking one switches to it. Left-click the icon to open the
  window; Quit is in the tray menu and in the window's menu.
- Quota is read every minute for every account, with the window open or in the tray.
- Automatic switching: when the account in use reaches its limit (5-hour or weekly at or below 5% by
  default), the app moves to the account with the most left. An almost-empty account counts as
  empty, so it never moves from 2% to 1%; when every account is empty it says which one refills
  first. With Antigravity open, a notice counts down (60 s by default) with Switch now and Not now;
  with it closed, only the sign-in changes.
- Settings: automatic switching on or off, the out-of-quota limit (1–50%), the warning time
  (0–600 s) and Start with Windows (starts in the tray).
- Starting the app again shows the running copy instead of opening a second one.
- Buttons are announced as buttons to screen readers and UI Automation.

## 1.0.0

A new app, written in Rust with a Slint UI.

- Account cards with orbit gauges for the weekly, 5-hour and Gemini limits, reset countdowns, and
  a details drawer with every limit and model.
- A guided add flow that notices when Antigravity finished signing in. Cancelling, a failure, or
  closing the window puts the previous sign-in back (or signs out when there was none).
- Switching asks before closing Antigravity and never freezes the window.
- Opening the account Antigravity is already signed in with just starts Antigravity, keeping the
  tokens it refreshed since the account was saved.
- Desktop shortcuts open a small window that confirms, switches and reports problems; the
  shortcut of the account already in use just starts Antigravity.
- Install a local copy with desktop and Start-menu shortcuts that carry the app's own icon.
- Quota uses the Windows proxy settings, including PAC scripts and automatic detection.
- When Antigravity's sign-in client cannot be found, it is looked for again on the next refresh.
- An expired sign-in is reported as such even when the sign-in client had to be found first.
- A diagnostic log without secrets in `%LOCALAPPDATA%\AntigravityAccountSwitcher\logs`.
- Compatible with what version 1.2 of the earlier switcher left on the PC: saved accounts, account
  shortcuts, the install folder and the sign-in client cache.
