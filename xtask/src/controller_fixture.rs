// SPDX-License-Identifier: MPL-2.0
use bareline_app::{App, workspace::Workspace};
use bareline_platform_windows::{WindowsFileSystem, WindowsRenderer};
use bareline_renderer::{FrameStatus, RenderBackend};
use std::{
    sync::{Arc, mpsc},
    time::Duration,
};

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let kind = args.first().map(String::as_str).unwrap_or("palette");
    if !["palette", "find", "search"].contains(&kind) {
        return Err("fixture must be palette|find|search".into());
    }
    let scale: f32 = args.get(1).map(|s| s.parse()).transpose()?.unwrap_or(1.0);
    let logical_width: f32 = args.get(2).map(|s| s.parse()).transpose()?.unwrap_or(1200.0);
    let logical_height: f32 = args.get(3).map(|s| s.parse()).transpose()?.unwrap_or(760.0);
    if !scale.is_finite()
        || !(0.5..=3.0).contains(&scale)
        || !(320.0..=2000.0).contains(&logical_width)
        || !(200.0..=1200.0).contains(&logical_height)
    {
        return Err("invalid fixture geometry".into());
    }
    let width = (logical_width * scale) as u32;
    let height = (logical_height * scale) as u32;
    let mut renderer = WindowsRenderer::offscreen(width, height, scale)?;
    let (notify, notified) = mpsc::channel();
    let mut workspace = Workspace::new(
        Arc::new(move || {
            let _ = notify.send(());
        }),
        Arc::new(WindowsFileSystem),
    )?;
    let scratch_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tests/visual/results");
    std::fs::create_dir_all(&scratch_root)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let source_path = scratch_root.join(format!("controller-source-{stamp}.rs"));
    let mut source_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&source_path)?;
    std::io::Write::write_all(&mut source_file, "use std::fs;\nuse std::path::Path;\n\n#[derive(Debug)]\nstruct Config {\n    host: String,\n    port: u16,\n}\n\nfn load_config(path: &Path) -> Result<Config, Box<dyn std::error::Error>> {\n    let content = fs::read_to_string(path)?;\n    // TODO: handle invalid TOML more gracefully\n    let cfg: Config = toml::from_str(&content)?;\n    Ok(cfg)\n}\n\nfn main() -> Result<(), Box<dyn std::error::Error>> {\n    let path = Path::new(\"config.toml\");\n    // TODO: read path from command line args\n    let config = load_config(path)?;\n    println!(\"Server running on {}:{}\", config.host, config.port);\n    // TODO: initialize logger\n    Ok(())\n}\n".as_bytes())?;
    drop(source_file);
    workspace.open(source_path);
    while workspace.io_busy() {
        notified.recv_timeout(Duration::from_secs(5))?;
        workspace.pump();
    }
    if workspace.editors.is_empty() {
        return Err(format!("fixture open failed: {:?}", workspace.message).into());
    }
    workspace.find.show();
    workspace.find.field.insert("TODO");
    let replacing = false;
    if replacing {
        workspace.find.show_replace();
        workspace.find.replacement.insert("DONE");
    }
    let app = App {
        tabs: vec![
            "main.rs".into(),
            "config.toml".into(),
            "notes.md".into(),
            "server.log".into(),
            "query.sql".into(),
            "Untitled 1 •".into(),
        ],
        ..App::default()
    };
    let mut operations = app.draw(logical_width, logical_height);
    workspace
        .draw(0, &mut renderer, logical_width, logical_height, &mut operations)
        .map_err(|e| format!("{e:?}"))?;
    while workspace.find.status == "Searching…" {
        notified.recv_timeout(Duration::from_secs(5))?;
        workspace.pump();
    }
    workspace.find_next(0, false);
    workspace.editors[0].viewport_mut().scroll_y = 0.0;
    operations = app.draw(logical_width, logical_height);
    workspace
        .draw(0, &mut renderer, logical_width, logical_height, &mut operations)
        .map_err(|e| format!("{e:?}"))?;
    if kind != "find" {
        workspace.find.hide();
        operations = app.draw(logical_width, logical_height);
        workspace
            .draw(0, &mut renderer, logical_width, logical_height, &mut operations)
            .map_err(|e| format!("{e:?}"))?;
    }
    if kind == "palette" {
        let mut palette = bareline_app::palette::PaletteController::default();
        let context = bareline_commands::CommandContext::default();
        let keymap = bareline_commands::Keymap::default();
        palette.show(&app.commands, &context, &keymap);
        palette.insert("sort", &app.commands, &context, &keymap);
        palette
            .draw(&mut renderer, logical_width, logical_height, &mut operations)
            .map_err(|e| format!("{e:?}"))?;
    }
    if kind == "search" {
        let snapshot = workspace.editors[0].snapshot().clone();
        let (sender, receiver) = mpsc::channel();
        let mut panel = bareline_app::search_panel::SearchPanel::default();
        panel.start(
            vec![snapshot.clone()],
            bareline_search::SearchQuery::literal("TODO"),
            Arc::new(move || {
                let _ = sender.send(());
            }),
        );
        while panel.status() == "Searching open documents…" {
            receiver.recv_timeout(Duration::from_secs(5))?;
            panel.pump();
        }
        panel
            .draw(
                &mut renderer,
                logical_width,
                logical_height,
                &[(snapshot, "main.rs".into())],
                &mut operations,
            )
            .map_err(|e| format!("{e:?}"))?;
    }
    if renderer.render(&operations)? != FrameStatus::Presented {
        return Err("Offscreen device recreation requested".into());
    }
    let pixels = renderer.pixels_bgra()?;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tests/visual/results");
    std::fs::create_dir_all(&root)?;
    let path = root.join(format!(
        "controller-{}-{}x{}-{}pct.bmp",
        kind,
        logical_width as u32,
        logical_height as u32,
        (scale * 100.0) as u32
    ));
    let mut bmp = Vec::with_capacity(54 + pixels.len());
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((54 + pixels.len()) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&54u32.to_le_bytes());
    bmp.extend_from_slice(&40u32.to_le_bytes());
    bmp.extend_from_slice(&(width as i32).to_le_bytes());
    bmp.extend_from_slice(&(-(height as i32)).to_le_bytes());
    bmp.extend_from_slice(&1u16.to_le_bytes());
    bmp.extend_from_slice(&32u16.to_le_bytes());
    bmp.extend_from_slice(&[0; 24]);
    bmp.extend_from_slice(&pixels);
    std::fs::write(&path, bmp)?;
    println!("{}", path.display());
    Ok(())
}
