// SPDX-License-Identifier: MPL-2.0
use bareline_app::{
    App,
    workspace::{Input, Workspace},
};
use bareline_platform_windows::{WindowsFileSystem, WindowsRenderer};
use bareline_renderer::{FrameStatus, RenderBackend};
use std::{
    sync::{Arc, mpsc},
    time::Duration,
};

pub fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let scale: f32 = args.first().map(|s| s.parse()).transpose()?.unwrap_or(1.0);
    let width = (1100.0 * scale) as u32;
    let height = (580.0 * scale) as u32;
    let mut renderer = WindowsRenderer::offscreen(width, height, scale)?;
    let (notify, notified) = mpsc::channel();
    let mut workspace = Workspace::new(
        Arc::new(move || {
            let _ = notify.send(());
        }),
        Arc::new(WindowsFileSystem),
    )?;
    workspace.new_document().map_err(|e| format!("{e:?}"))?;
    workspace.editors[0].enqueue(Input::Insert("use std::fs;\nuse std::path::Path;\n\n#[derive(Debug)]\nstruct Config {\n    host: String,\n    port: u16,\n}\n\nfn load_config(path: &Path) -> Result<Config, Box<dyn std::error::Error>> {\n    let content = fs::read_to_string(path)?;\n    // TODO: handle invalid TOML more gracefully\n    let cfg: Config = toml::from_str(&content)?;\n    Ok(cfg)\n}\n\nfn main() -> Result<(), Box<dyn std::error::Error>> {\n    let path = Path::new(\"config.toml\");\n    // TODO: read path from command line args\n    let config = load_config(path)?;\n    println!(\"Server running on {}:{}\", config.host, config.port);\n    // TODO: initialize logger\n    Ok(())\n}\n".into()));
    while workspace.editors[0].busy() {
        notified.recv_timeout(Duration::from_secs(5))?;
        workspace.pump();
    }
    workspace.find.show();
    workspace.find.field.insert("TODO");
    let replacing = args.get(1).is_some_and(|value| value == "replace");
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
    let mut operations = app.draw(1100.0, 580.0);
    workspace
        .draw(0, &mut renderer, 1100.0, 580.0, &mut operations)
        .map_err(|e| format!("{e:?}"))?;
    while workspace.find.status == "Searching…" {
        notified.recv_timeout(Duration::from_secs(5))?;
        workspace.pump();
    }
    workspace.find_next(0, false);
    workspace.editors[0].viewport_mut().scroll_y = 0.0;
    operations = app.draw(1100.0, 580.0);
    workspace
        .draw(0, &mut renderer, 1100.0, 580.0, &mut operations)
        .map_err(|e| format!("{e:?}"))?;
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
        "{}-{}pct.bmp",
        if replacing { "replace" } else { "find" },
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
