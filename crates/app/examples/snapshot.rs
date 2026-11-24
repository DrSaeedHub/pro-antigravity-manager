//! Renders a screen with demo data and saves it as a PNG, for design review.
//!
//! `cargo run -p app --example snapshot -- <out.png> [scene] [width] [height]`
//! Scenes: main, empty, add, sign-in, switch, remove, details, help, about, settings, launch,
//! launch-failed, notice, notice-empty, notice-switched, notice-tray.
#![allow(clippy::print_stderr, reason = "a developer tool reports to the console")]

use std::error::Error;
use std::fs::File;
use std::io::BufWriter;
use std::rc::Rc;
use std::time::Duration;

use app::ui::{
    AccountView, AddStep, AppWindow, CardState, DetailGroup, DetailRow, Details, Gauge,
    LaunchStage, LaunchWindow, NoticeKind, NoticeWindow, RowKind, SheetKind, Toast, ToastKind,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

fn gauge(fraction: f32, resets: &str) -> Gauge {
    Gauge {
        present: true,
        fraction,
        percent: SharedString::from(format!("{:.0}%", fraction * 100.0)),
        resets: resets.into(),
        low: fraction < 0.2,
    }
}

fn account(name: &str, email: &str, plan: &str, active: bool) -> AccountView {
    AccountView {
        name: name.into(),
        initial: app::view::initial(name),
        email: email.into(),
        plan: plan.into(),
        active,
        state: CardState::Ready,
        ..AccountView::default()
    }
}

fn demo() -> Vec<AccountView> {
    let mut work = account("work", "ada.work@example.com", "GOOGLE AI PRO", true);
    work.five_hour = gauge(0.82, "2h 15m");
    work.weekly = gauge(0.41, "3d 4h");
    work.gemini = gauge(1.0, "");

    let mut personal = account("personal", "ada@example.com", "FREE", false);
    personal.five_hour = gauge(0.12, "48m");
    personal.weekly = gauge(0.67, "5d 2h");
    personal.gemini = gauge(0.55, "6h 10m");

    let mut lab = account("lab-2", "lab.two@example.com", "", false);
    lab.state = CardState::Failed;
    lab.problem_code = "quota.sign_in_expired".into();

    let mut fresh = account("research", "", "", false);
    fresh.state = CardState::Loading;

    vec![work, personal, lab, fresh]
}

fn row(label: &str, kind: RowKind, fraction: f32, note: &str) -> DetailRow {
    DetailRow {
        label: label.into(),
        kind,
        fraction,
        percent: format!("{:.0}%", fraction * 100.0).into(),
        note: note.into(),
        low: fraction < 0.2,
    }
}

fn details() -> Details {
    let limits = vec![
        row("5-hour limit", RowKind::FiveHour, 0.82, "2h 15m · 16:45"),
        row("Weekly limit", RowKind::Weekly, 0.41, "3d 4h · Thu 27 Nov 08:00"),
    ];
    let models = vec![
        row("Gemini 3 Pro", RowKind::Gemini, 1.0, ""),
        row("Gemini 3 Flash", RowKind::Gemini, 0.9, "4h 2m · 18:32"),
        row("Claude Sonnet 4.5", RowKind::Other, 0.82, "2h 15m · 16:45"),
        row("Claude Opus 4.5", RowKind::Other, 0.15, "2h 15m · 16:45"),
        row("GPT-OSS 120B", RowKind::Other, 0.82, "2h 15m · 16:45"),
        row("Extra model 1", RowKind::Other, 0.5, ""),
        row("Extra model 2", RowKind::Other, 0.5, ""),
        row("Extra model 3", RowKind::Other, 0.5, ""),
        row("Extra model 4", RowKind::Other, 0.5, ""),
        row("Extra model 5", RowKind::Other, 0.5, ""),
        row("Extra model 6", RowKind::Other, 0.5, ""),
        row("Extra model 7", RowKind::Other, 0.5, ""),
        row("Extra model 8", RowKind::Other, 0.5, ""),
    ];
    Details {
        name: "work".into(),
        email: "ada.work@example.com".into(),
        plan: "GOOGLE AI PRO".into(),
        updated: "14:32".into(),
        groups: ModelRc::new(VecModel::from(vec![DetailGroup {
            name: "Claude and GPT models".into(),
            rows: ModelRc::new(VecModel::from(limits)),
        }])),
        models: ModelRc::new(VecModel::from(models)),
        ..Details::default()
    }
}

fn save(
    path: &str,
    buffer: &slint::SharedPixelBuffer<slint::Rgba8Pixel>,
) -> Result<(), Box<dyn Error>> {
    let mut encoder =
        png::Encoder::new(BufWriter::new(File::create(path)?), buffer.width(), buffer.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(buffer.as_bytes())?;
    Ok(())
}

fn capture(window: &slint::Window, out: &str) {
    match window.take_snapshot() {
        Ok(buffer) => {
            if let Err(error) = save(out, &buffer) {
                eprintln!("could not save {out}: {error}");
            }
        }
        Err(error) => eprintln!("snapshot failed: {error}"),
    }
    let _ = slint::quit_event_loop();
}

fn main_window(scene: &str, width: f32, height: f32, out: String) -> Result<(), Box<dyn Error>> {
    let window = AppWindow::new()?;
    window.window().set_size(slint::LogicalSize::new(width, height));
    window.set_version("1.0.0".into());
    window.set_loaded(true);
    let accounts = if scene == "empty" { Vec::new() } else { demo() };
    window.set_accounts(ModelRc::from(Rc::new(VecModel::from(accounts))));
    window.set_updated_at("14:32".into());
    window.set_failed_count(1);
    match scene {
        "add" => {
            window.set_add_name("work-2".into());
            window.set_sheet(SheetKind::Add);
        }
        "sign-in" => {
            window.set_add_name("work-2".into());
            window.set_add_step(AddStep::SignIn);
            window.set_add_detected(true);
            window.set_add_email("second.work@example.com".into());
            window.set_sheet(SheetKind::Add);
        }
        "switch" => {
            window.set_target_name("personal".into());
            window.set_sheet(SheetKind::Switch);
        }
        "remove" => {
            window.set_target_name("lab-2".into());
            window.set_sheet(SheetKind::Remove);
        }
        "details" => {
            window.set_details(details());
            window.set_drawer_open(true);
        }
        "help" => window.set_sheet(SheetKind::Help),
        "about" => window.set_sheet(SheetKind::About),
        "settings" => window.set_sheet(SheetKind::Settings),
        _ => window.set_toast(Toast {
            serial: 1,
            kind: ToastKind::Success,
            code: "switched".into(),
            name: "work".into(),
            detail: SharedString::new(),
        }),
    }
    let weak = window.as_weak();
    slint::Timer::single_shot(Duration::from_millis(1800), move || {
        if let Some(window) = weak.upgrade() {
            capture(window.window(), &out);
        }
    });
    window.run()?;
    Ok(())
}

fn launch_window(scene: &str, out: String) -> Result<(), Box<dyn Error>> {
    let window = LaunchWindow::new()?;
    window.set_account("personal".into());
    if scene == "launch-failed" {
        window.set_stage(LaunchStage::Failed);
        window.set_problem_code("antigravity.not_installed".into());
    } else {
        window.set_stage(LaunchStage::Confirm);
    }
    let weak = window.as_weak();
    slint::Timer::single_shot(Duration::from_millis(900), move || {
        if let Some(window) = weak.upgrade() {
            capture(window.window(), &out);
        }
    });
    window.run()?;
    Ok(())
}

fn notice_window(scene: &str, out: String) -> Result<(), Box<dyn Error>> {
    let window = NoticeWindow::new()?;
    match scene {
        "notice-empty" => {
            window.set_kind(NoticeKind::AllEmpty);
            window.set_to("personal".into());
            window.set_refill("16:45".into());
        }
        "notice-switched" => {
            window.set_kind(NoticeKind::SignedIn);
            window.set_from("work".into());
            window.set_to("personal".into());
        }
        "notice-tray" => window.set_kind(NoticeKind::InTray),
        _ => {
            window.set_kind(NoticeKind::Countdown);
            window.set_from("work".into());
            window.set_to("personal".into());
            window.set_left("67%".into());
            window.set_seconds_total(60);
            window.set_seconds_left(42);
        }
    }
    let weak = window.as_weak();
    slint::Timer::single_shot(Duration::from_millis(1500), move || {
        if let Some(window) = weak.upgrade() {
            capture(window.window(), &out);
        }
    });
    window.run()?;
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "snapshot.png".to_owned());
    let scene = args.next().unwrap_or_else(|| "main".to_owned());
    let width: f32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(1180.0);
    let height: f32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(780.0);

    app::window::select_backend()?;
    if scene.starts_with("notice") {
        return notice_window(&scene, out);
    }
    if scene.starts_with("launch") {
        launch_window(&scene, out)
    } else {
        main_window(&scene, width, height, out)
    }
}
