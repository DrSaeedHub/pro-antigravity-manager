# Changelog

## 1.3.0

A full pass over the interface.

- Cards show all four limits: a small table with Claude and GPT and Gemini as rows and 5-hour and
  weekly as columns, each with its percent and the time until it refills. The orbit gauge has one
  orbit per limit, and a coloured dot ties each number to its orbit (5-hour limits warm, weekly
  ones cool; Gemini's 5-hour limit has a colour of its own, pink).
- Cards never overflow. Before, at some window widths a card was narrower than its content, so
  countdowns and the ACTIVE tag spilled out of it. The grid now never makes a card narrower than
  its content needs (checked at every width from 900 to 2000 pixels), and long names, emails and
  plans shorten with an ellipsis instead of pushing anything out.
- The satellites sit exactly on the ends of their orbits. Slint draws a path shrunk by its stroke
  width unless told otherwise, so each arc ran about two pixels inside its track and off its
  satellite. The arcs, the logo and the spinner now draw on their tracks' centre lines.
- New icons throughout, drawn as SVG on one grid with one stroke, sharp at every screen scale:
  a swap icon for Switch, an external-link icon for Open, a gauge for quota details, a monitor for
  desktop shortcuts, sliders for Settings, a power icon for Quit, and redrawn refresh, download,
  help, info, warning, trash, plus, minus and window buttons.
- Text sits on the centre line of its button, tag or row. Each font places its capitals off the
  centre of its line box (Bahnschrift above it, Segoe UI Variable below), so labels rode a pixel
  above their icons and names beside numbers sat lower than them; labels now correct for their
  font and line up to half a pixel at 100%, 125% and 150%.
- Resizing the window no longer drags the closed quota details panel, or a hidden notice at the
  bottom, into view for a moment. Dialogs stay centred while the window is resized.
- Maximize follows Windows: it maximizes and restores through the window system and shows
  Maximize or Restore as Windows sees the window, also after Win+Up, snapping or a double-click.
- A scroll bar appears beside the cards when they do not all fit; drag it or click its track.

## 1.2.2

- Quota matches what Antigravity shows. Google now has two groups of limits, Gemini models and
  Claude and GPT models, each with a 5-hour and a weekly limit, and it sends the Gemini group only
  to recent Antigravity versions on Antigravity's own server. The app now reads that server
  (`daily-cloudcode-pa`) and names the installed Antigravity's version in its requests. Before, it
  asked the other server, which reports every limit as full, so cards could say 100% while
  Antigravity said 95%.
- Cards show Claude and GPT's 5-hour and weekly limits and Gemini's tighter limit (labelled
  5-hour or weekly). The details drawer lists both groups with both limits, Gemini first as in
  Antigravity, and every model. The tray menu shows each account's Claude and GPT and Gemini
  quota.
- Automatic switching looks at both groups: when one runs out on the account in use, the app moves
  to the account with the most left in that group, so an account with plenty of Gemini but no
  Claude quota is not picked when Claude is what ran out.
- The window's Minimize button works every time. Before, it only worked once: after the window was
  restored from the taskbar, the button did nothing.

## 1.2.1

- Uninstalling opens the app's own window instead of Windows' bare "Are you sure" box. It says
  what is removed (the app, its shortcuts, every account's desktop shortcut and Start with
  Windows) and what stays (Antigravity and the account it is signed in with), and offers two
  switches, both off unless you turn them on: also delete the saved accounts, and also delete
  settings and logs. Cancel keeps everything. A scripted uninstall asks nothing:
  `unins000.exe /VERYSILENT`, with `/DELETEACCOUNTS` and `/DELETESETTINGS` when wanted.
- An installed copy that updates itself now brings its whole installation along: after
  restarting on the new version it downloads that version's installer, runs it without a window
  and opens again, so the uninstaller and the Installed apps entry always match the app. The
  release feed lists the installer for this.
- Corner notices (the countdown when quota runs out, every account being empty, a new version)
  always sit wholly on screen, in the bottom-right corner above the taskbar, on top of other
  windows. Before, a notice could hang off the bottom of the screen when Windows applied the
  screen's scaling after the notice had been placed. The main window and the shortcut window are
  moved back onto the screen they are on when part of them would be off it.

## 1.2.0

- Updates: the app looks for a new version on GitHub soon after it starts and then every minute,
  with the window open or in the tray. A new version shows as an Update button at the top, an
  item in the tray menu, a line in About and, once per version while the window is hidden, a
  corner notice. One click downloads it with progress (Cancel stops it), checks its size, its
  SHA-256 and that it starts and reports the right version, puts it in place and restarts on it.
  Nothing is installed without that click. Automatic checks can be turned off in Settings; About
  has Check for updates.
- Opening the app while it runs always brings the running copy's window back, restored from
  minimized or from the tray and in front, and never opens a second copy. That holds even when an
  earlier copy hangs while closing and still holds the single-copy lock (Windows can leave one
  behind): the new start takes over instead of running beside it. A copy that is alive but does
  not respond is left alone, and a note says so.
- About and the tray menu link to the project's GitHub page.
- Every release has an installer and a portable zip. The installer needs no administrator rights,
  adds Start menu and desktop shortcuts (Start with Windows is a choice), and lists the app in
  Installed apps; it closes a running copy first. Uninstalling removes the program and the
  shortcuts the app made, and keeps settings and saved accounts.
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
