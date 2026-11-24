//! The demonstration the README's screenshots come from: the real controller, windows and tray
//! icon, driven through their accessible buttons, with pretend Windows parts (credentials in
//! memory, a pretend Antigravity, shortcuts that are only recorded, every folder under a scratch
//! root) and the pretend Google quota service and GitHub release feed that `tools/demo/demo.py`
//! runs. Nothing real on this PC is read or changed.
//!
//! `cargo run -p app --example demo -- <session.json>`
//!
//! `tools/demo/demo.py` writes the session file: the scratch root, the pretend Google service,
//! the saved accounts, and the steps to take. Each step is an object with one of these keys:
//!
//! - `wait`: `"quota"` (every card has its reading) or a number of milliseconds;
//! - `click`: an accessible label, with `nth` among equal labels and `in` (`main`, `notice`,
//!   `launch`), and `within` (`{id | type | label, has}`: only inside the element that shows the
//!   text `has`); the click is a pointer press and release at the element's center;
//! - `scroll`: an element (`{id | type | label, nth}`), scrolled by `by` logical pixels;
//! - `type`: text typed into the focused field;
//! - `sign_in`: `{email, refresh}`, what Antigravity writes when someone finishes signing in;
//! - `launch`: an account name: the window its desktop shortcut opens;
//! - `show`: brings the main window back;
//! - `external`: a task for `demo.py`, which answers on standard input when it is done;
//! - `shot`: a file name, with `in` and either `crop` (`[x, y, width, height]`, logical pixels)
//!   or `around` (a list of `{id | type | label, nth}` elements, plus `pad`).
//!
//! With `"serve": true` the app keeps running after the last step; with `"hidden": true` the main
//! window starts hidden, as it does when the app starts in the tray.
#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "a developer tool that talks to demo.py on the console"
)]

use std::cell::RefCell;
use std::collections::VecDeque;
use std::error::Error;
use std::fs::{self, File};
use std::io::{BufRead, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use app::bootstrap::{self, Parts, Services};
use app::controller::{self, Controller};
use app::ui::{AppWindow, CardState, LaunchWindow};
use i_slint_backend_testing::ElementHandle;
use serde_json::{Value, json};
use slint::platform::WindowEvent;
use slint::{ComponentHandle, Model};

/// How long a `wait: "quota"` may take.
const QUOTA_LIMIT: Duration = Duration::from_secs(30);

type Failure = Box<dyn Error>;

fn text(node: &Value, key: &str) -> String {
    node.get(key).and_then(Value::as_str).unwrap_or_default().to_owned()
}

fn flag(node: &Value, key: &str) -> bool {
    node.get(key).and_then(Value::as_bool).unwrap_or(false)
}

#[allow(clippy::cast_possible_truncation, reason = "small pixel and count values")]
fn number(node: &Value, key: &str) -> Option<f32> {
    node.get(key).and_then(Value::as_f64).map(|n| n as f32)
}

#[allow(clippy::cast_possible_truncation, reason = "small counts")]
fn index(node: &Value, key: &str) -> usize {
    node.get(key).and_then(Value::as_u64).unwrap_or(0) as usize
}

/// A saved sign-in as Antigravity writes it.
fn sign_in(email: &str, refresh: &str) -> Vec<u8> {
    json!({ "id_token_email": email, "token": { "refresh_token": refresh } })
        .to_string()
        .into_bytes()
}

/// A pretend Antigravity install: the executable and a language server that carries the
/// installed-app sign-in client the quota service looks for.
fn pretend_antigravity(root: &Path) -> Result<(), Failure> {
    let home = root.join("LocalAppData").join("Programs").join("antigravity");
    fs::create_dir_all(home.join("resources").join("bin"))?;
    fs::write(home.join("Antigravity.exe"), b"pretend")?;
    // Built at run time so the source never holds a string shaped like a real client secret.
    let client = format!(
        "...{}-{}.apps.googleusercontent.com...GOCSPX-{}...",
        "100000000001",
        "demo".repeat(8),
        "Demo".repeat(7)
    );
    fs::write(home.join("resources").join("bin").join("language_server_windows_x64.exe"), client)?;
    Ok(())
}

struct Session {
    root: PathBuf,
    api: String,
    out: PathBuf,
    now: jiff::Timestamp,
    serve: bool,
    main: bool,
    hidden: bool,
    size: (f32, f32),
    only: Vec<String>,
}

impl Session {
    fn read(spec: &Value) -> Result<Self, Failure> {
        let size = spec.get("window").cloned().unwrap_or(Value::Null);
        Ok(Self {
            root: PathBuf::from(text(spec, "root")),
            api: text(spec, "api"),
            out: PathBuf::from(text(spec, "out")),
            now: text(spec, "now").parse()?,
            serve: flag(spec, "serve"),
            main: spec.get("main").and_then(Value::as_bool).unwrap_or(true),
            hidden: flag(spec, "hidden"),
            size: (
                number(&size, "width").unwrap_or(1260.0),
                number(&size, "height").unwrap_or(720.0),
            ),
            only: spec
                .get("only")
                .and_then(Value::as_array)
                .map(|names| names.iter().filter_map(Value::as_str).map(str::to_owned).collect())
                .unwrap_or_default(),
        })
    }

    fn parts(&self, store: Arc<vault::MemoryStore>, antigravity_open: bool) -> Parts {
        Parts {
            store,
            host: Arc::new(antigravity::FakeHost::with_processes(usize::from(antigravity_open))),
            folders: Arc::new(kernel::FixedFolders::new(&self.root)),
            shell: Arc::new(desktop::FakeShell::new()),
            secret_box: Arc::new(quota::FakeBox),
            proxy: Arc::new(net::NoProxy),
            clock: Arc::new(kernel::ManualClock::new(self.now, jiff::tz::TimeZone::UTC)),
            current_exe: self.root.join("AntigravityAccountSwitcher.exe"),
            quota_settings: quota::QuotaSettings {
                token_url: format!("{}/token", self.api),
                api_base: format!("{}/v1internal:", self.api),
                ..quota::QuotaSettings::default()
            },
            // The pretend GitHub of demo.py; an update never gets past Cancel in the shot list.
            update_settings: update::UpdateSettings {
                repository: format!("{}/gh/owner/repo", self.api),
                ..update::UpdateSettings::default()
            },
            probe: Arc::new(update::FakeProbe::failing("the demo installs nothing")),
        }
    }
}

enum Wait {
    Nothing,
    Until(Instant),
    Quota(Instant),
    Answer,
}

struct Runner {
    session: Session,
    steps: VecDeque<Value>,
    wait: Wait,
    services: Services,
    store: Arc<vault::MemoryStore>,
    main: Option<AppWindow>,
    controller: Option<Controller>,
    launch: Option<LaunchWindow>,
    answers: Receiver<String>,
    failures: usize,
    done: bool,
}

impl Runner {
    fn fail(&mut self, step: &Value, why: &str) {
        eprintln!("FAILED {step}: {why}");
        self.failures += 1;
    }

    fn with_window<R>(&self, step: &Value, act: impl FnOnce(&slint::Window) -> R) -> Option<R> {
        match text(step, "in").as_str() {
            "notice" => controller::shown_notice().map(|notice| act(notice.window())),
            "launch" => self.launch.as_ref().map(|window| act(window.window())),
            _ => self.main.as_ref().map(|window| act(window.window())),
        }
    }

    /// No leftover hover from wherever the real mouse happens to be.
    fn forget_pointer(&self) {
        for place in ["main", "launch", "notice"] {
            self.with_window(&json!({ "in": place }), |window| {
                window.dispatch_event(WindowEvent::PointerExited);
            });
        }
    }

    fn elements(
        &self,
        step: &Value,
        finder: &dyn Fn(&dyn ElementSource) -> Vec<ElementHandle>,
    ) -> Vec<ElementHandle> {
        match text(step, "in").as_str() {
            "notice" => controller::shown_notice().map(|n| finder(&n)).unwrap_or_default(),
            "launch" => self.launch.as_ref().map(|w| finder(w)).unwrap_or_default(),
            _ => self.main.as_ref().map(|w| finder(w)).unwrap_or_default(),
        }
    }

    fn quota_settled(&self) -> bool {
        let Some(window) = &self.main else { return true };
        let accounts = window.get_accounts();
        window.get_loaded()
            && !window.get_refreshing()
            && accounts.iter().all(|account| account.state != CardState::Loading)
    }

    fn tick(&mut self) {
        match self.wait {
            Wait::Until(at) if Instant::now() < at => return,
            Wait::Quota(limit) if !self.quota_settled() => {
                if Instant::now() > limit {
                    self.fail(&json!("wait quota"), "quota never settled");
                    self.wait = Wait::Nothing;
                }
                return;
            }
            Wait::Answer => match self.answers.try_recv() {
                Ok(_) => self.wait = Wait::Nothing,
                Err(_) => return,
            },
            _ => self.wait = Wait::Nothing,
        }
        let Some(step) = self.steps.pop_front() else {
            self.finish();
            return;
        };
        if let Err(error) = self.run(&step) {
            self.fail(&step, &error.to_string());
        }
    }

    fn finish(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        if self.session.serve {
            println!("SERVING");
            return;
        }
        if let Some(window) = &self.main {
            window.invoke_quit();
        } else {
            let _ = slint::quit_event_loop();
        }
    }

    fn run(&mut self, step: &Value) -> Result<(), Failure> {
        if let Some(wait) = step.get("wait") {
            self.forget_pointer();
            self.wait = match wait {
                Value::String(what) if what == "quota" => Wait::Quota(Instant::now() + QUOTA_LIMIT),
                Value::Number(ms) => {
                    Wait::Until(Instant::now() + Duration::from_millis(ms.as_u64().unwrap_or(0)))
                }
                _ => return Err("wait: \"quota\" or milliseconds".into()),
            };
        } else if step.get("click").is_some() {
            // A real press and release at the element's center, as the mouse would send them
            // (popups close, hover and press states run), not just the button's action.
            let label = text(step, "click");
            let found = match step.get("within") {
                // Only inside the element (a card, say) that shows the text `has`.
                Some(within) => {
                    let has = text(within, "has");
                    self.elements(step, &|root| root.find(within))
                        .into_iter()
                        .filter(|scope| {
                            scope
                                .query_descendants()
                                .match_accessible_label(has.as_str())
                                .find_first()
                                .is_some()
                        })
                        .flat_map(|scope| {
                            scope
                                .query_descendants()
                                .match_accessible_label(label.as_str())
                                .find_all()
                        })
                        .collect()
                }
                None => self.elements(step, &|root| root.by_label(&label)),
            };
            let element = found
                .get(index(step, "nth"))
                .ok_or_else(|| format!("{} element(s) with that label", found.len()))?;
            element.mock_single_click(slint::platform::PointerEventButton::Left);
        } else if let Some(target) = step.get("scroll") {
            let found = self.elements(step, &|root| root.find(target));
            let element = found.get(index(target, "nth")).ok_or("nothing to scroll")?;
            element.scroll(0.0, number(step, "by").unwrap_or(0.0));
        } else if step.get("type").is_some() {
            let typed = text(step, "type");
            self.with_window(step, |window| {
                for character in typed.chars() {
                    let key: slint::SharedString = character.to_string().into();
                    window.dispatch_event(WindowEvent::KeyPressed { text: key.clone() });
                    window.dispatch_event(WindowEvent::KeyReleased { text: key });
                }
            })
            .ok_or("no window")?;
        } else if let Some(who) = step.get("sign_in") {
            let canonical = self.services.accounts.settings().canonical.clone();
            self.store.insert(&canonical, &sign_in(&text(who, "email"), &text(who, "refresh")));
        } else if step.get("launch").is_some() {
            self.launch = app::cli::open(&self.services, &text(step, "launch"))?;
            if self.launch.is_none() {
                return Err("the shortcut had nothing to do".into());
            }
        } else if step.get("show").is_some() {
            self.controller.as_ref().ok_or("no main window")?.show();
        } else if step.get("external").is_some() {
            println!("EXTERNAL {}", text(step, "external"));
            std::io::stdout().flush()?;
            self.wait = Wait::Answer;
        } else if step.get("shot").is_some() {
            self.shot(step)?;
        } else {
            return Err("unknown step".into());
        }
        Ok(())
    }

    fn shot(&mut self, step: &Value) -> Result<(), Failure> {
        let name = text(step, "shot");
        if !self.session.only.is_empty() && !self.session.only.contains(&name) {
            return Ok(());
        }
        let rect = match (step.get("crop"), step.get("around")) {
            (Some(Value::Array(crop)), _) => {
                let at = |i: usize| crop.get(i).and_then(Value::as_f64).unwrap_or(0.0);
                Some((at(0), at(1), at(2), at(3)))
            }
            (_, Some(Value::Array(targets))) => {
                let pad = f64::from(number(step, "pad").unwrap_or(0.0));
                let mut union: Option<(f64, f64, f64, f64)> = None;
                for target in targets {
                    let found = self.elements(step, &|root| root.find(target));
                    let element = found
                        .get(index(target, "nth"))
                        .ok_or_else(|| format!("{target} not found"))?;
                    let (at, size) = (element.absolute_position(), element.size());
                    let (x0, y0) = (f64::from(at.x), f64::from(at.y));
                    let (x1, y1) = (x0 + f64::from(size.width), y0 + f64::from(size.height));
                    union = Some(match union {
                        None => (x0, y0, x1, y1),
                        Some((a, b, c, d)) => (a.min(x0), b.min(y0), c.max(x1), d.max(y1)),
                    });
                }
                union.map(|(x0, y0, x1, y1)| {
                    (x0 - pad, y0 - pad, x1 - x0 + 2.0 * pad, y1 - y0 + 2.0 * pad)
                })
            }
            _ => None,
        };
        let (buffer, scale) = self
            .with_window(step, |window| (window.take_snapshot(), window.scale_factor()))
            .ok_or("that window is not shown")?;
        let (buffer, scale) = (buffer?, f64::from(scale));
        let (width, height) = (f64::from(buffer.width()), f64::from(buffer.height()));
        let (x, y, w, h) = rect.map_or((0.0, 0.0, width, height), |(x, y, w, h)| {
            let x0 = (x * scale).round().clamp(0.0, width);
            let y0 = (y * scale).round().clamp(0.0, height);
            let x1 = ((x + w) * scale).round().clamp(x0, width);
            let y1 = ((y + h) * scale).round().clamp(y0, height);
            (x0, y0, x1 - x0, y1 - y0)
        });
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "clamped pixels"
        )]
        let (x, y, w, h) = (x as usize, y as usize, w as usize, h as usize);
        let stride = buffer.width() as usize;
        let mut pixels = Vec::with_capacity(w * h * 4);
        for row in y..y + h {
            let start = (row * stride + x) * 4;
            pixels.extend_from_slice(&buffer.as_bytes()[start..start + w * 4]);
        }
        fs::create_dir_all(&self.session.out)?;
        let path = self.session.out.join(format!("{name}.png"));
        let mut encoder = png::Encoder::new(
            BufWriter::new(File::create(&path)?),
            u32::try_from(w)?,
            u32::try_from(h)?,
        );
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&pixels)?;
        println!("SHOT {name} {w}x{h}");
        Ok(())
    }
}

/// Where elements are looked up: a component.
trait ElementSource {
    fn by_label(&self, label: &str) -> Vec<ElementHandle>;
    fn find(&self, target: &Value) -> Vec<ElementHandle>;
}

impl<T: ComponentHandle> ElementSource for T {
    fn by_label(&self, label: &str) -> Vec<ElementHandle> {
        ElementHandle::find_by_accessible_label(self, label).collect()
    }

    fn find(&self, target: &Value) -> Vec<ElementHandle> {
        if let Some(id) = target.get("id").and_then(Value::as_str) {
            ElementHandle::find_by_element_id(self, id).collect()
        } else if let Some(kind) = target.get("type").and_then(Value::as_str) {
            ElementHandle::find_by_element_type_name(self, kind).collect()
        } else {
            self.by_label(&text(target, "label"))
        }
    }
}

fn main() -> Result<(), Failure> {
    let path = std::env::args().nth(1).ok_or("usage: demo <session.json>")?;
    let spec: Value = serde_json::from_str(&fs::read_to_string(path)?)?;
    let session = Session::read(&spec)?;
    fs::create_dir_all(&session.root)?;
    pretend_antigravity(&session.root)?;

    let store = Arc::new(vault::MemoryStore::new());
    let services = bootstrap::build(session.parts(store.clone(), flag(&spec, "antigravity_open")))?;
    let settings = services.accounts.settings().clone();
    for account in spec.get("accounts").and_then(Value::as_array).into_iter().flatten() {
        let blob = sign_in(&text(account, "email"), &text(account, "refresh"));
        store.insert(&settings.slot(&text(account, "name")), &blob);
        if flag(account, "signed_in") {
            store.insert(&settings.canonical, &blob);
        }
    }

    app::window::select_backend()?;
    let (main, controller) = if session.main {
        let window = AppWindow::new()?;
        window.window().set_size(slint::LogicalSize::new(session.size.0, session.size.1));
        window.window().set_position(slint::PhysicalPosition::new(0, 0));
        let controller = Controller::new(&window, services.clone());
        if !session.hidden {
            controller.show();
        }
        (Some(window), Some(controller))
    } else {
        (None, None)
    };

    let (send, answers) = channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if send.send(line).is_err() {
                break;
            }
        }
    });

    let steps = spec.get("steps").and_then(Value::as_array).cloned().unwrap_or_default();
    let runner = Rc::new(RefCell::new(Runner {
        session,
        steps: steps.into(),
        wait: Wait::Nothing,
        services,
        store,
        main,
        controller,
        launch: None,
        answers,
        failures: 0,
        done: false,
    }));
    let timer = slint::Timer::default();
    {
        let runner = runner.clone();
        timer.start(slint::TimerMode::Repeated, Duration::from_millis(40), move || {
            if let Ok(mut runner) = runner.try_borrow_mut() {
                runner.tick();
            }
        });
    }
    slint::run_event_loop_until_quit()?;
    timer.stop();
    controller::release();
    let failures = runner.borrow().failures;
    drop(runner);
    if failures > 0 {
        return Err(format!("{failures} step(s) failed").into());
    }
    Ok(())
}
