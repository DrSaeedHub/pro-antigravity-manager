# Security policy

## Reporting a problem

Please do not publish credentials, account names, emails or logs in a public issue. Use the
repository's private security advisory feature, with the affected version, the steps to reproduce,
and the impact. Remove personal data from screenshots and logs first.

## How the app treats secrets

- Saved accounts are Windows Credential Manager entries (`antigravity-switcher:<name>`), protected
  by Windows for the current user. The app copies Antigravity's credential (`gemini:antigravity`)
  as a whole and never changes its content or writes it to a file.
- Access tokens live in memory only. Refresh tokens are sent only to `oauth2.googleapis.com`;
  quota requests go only to `cloudcode-pa.googleapis.com`, through the Windows proxy settings.
- Antigravity's sign-in client is read from its install and cached with DPAPI (current user) in
  `%LOCALAPPDATA%\AntigravityAccountSwitcher\oauth-client.bin`.
- The log in `%LOCALAPPDATA%\AntigravityAccountSwitcher\logs` never contains tokens: secret values
  are wrapped in a type that prints as `***`.
- The app reads no environment variables and runs without administrator rights.
