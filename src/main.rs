mod audio;
mod auto_privacy;
mod background_blur;
mod capture_idle;
mod controller;
mod exposure;
mod fluency_check;
mod hardware_check;
mod hid;
mod meter;
mod model;
mod preview;
mod quad;
mod recording;
mod sharing_check;
mod sharing_usage;
mod transform;
mod ui;
mod uvc;
mod verification;
mod video_activity;
mod virtual_camera;
mod virtual_check;
mod workflow_check;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    // Fresh CMIO metadata process avoids the installed extension's property
    // cache. This path never initializes UI, camera capture or a producer.
    if args.iter().any(|a| a == "--read-virtual-usage") {
        println!(
            "{}",
            serde_json::to_string(&virtual_camera::raw_consumer_count())?
        );
        return Ok(());
    }
    if let Some(i) = args.iter().position(|a| a == "--blur-image") {
        let input = args.get(i + 1).ok_or("Informe imagem de entrada")?;
        let output = args.get(i + 2).ok_or("Informe imagem de saída")?;
        let report = args.get(i + 3).ok_or("Informe relatório JSON")?;
        let image = image::open(input)?.into_rgb8();
        let (w, h) = image.dimensions();
        let stats = background_blur::benchmark(w, h, image.as_raw(), 0.65, 12)?;
        let mut processor = background_blur::Processor::new()?;
        let pixels = processor.apply(w, h, image.into_raw(), 0.65)?;
        image::save_buffer(output, &pixels, w, h, image::ColorType::Rgb8)?;
        model::save_json(std::path::Path::new(report), &stats)?;
        return Ok(());
    }
    for (flag, run) in [
        (
            "--verify-streams",
            verification::streams as fn(&std::path::Path) -> Result<(), String>,
        ),
        ("--verify-hardware", verification::hardware),
        ("--verify-workflows", workflow_check::run),
        ("--verify-privacy", verification::privacy),
        ("--verify-virtual", virtual_check::run),
        ("--verify-sharing", virtual_check::persistent_session),
        ("--verify-fluency", fluency_check::run),
        ("--consume-virtual", sharing_check::virtual_consumer),
        ("--consume-physical", sharing_check::physical_consumer),
        ("--verify-startup", verification::startup),
        ("--verify-stress", workflow_check::stress),
    ] {
        if let Some(i) = args.iter().position(|a| a == flag) {
            let path = args.get(i + 1).ok_or("Informe o caminho JSON")?;
            verification::launch(run, std::path::PathBuf::from(path))?;
            return Ok(());
        }
    }
    if args.iter().any(|a| a == "--hardware-test") {
        println!("{}", serde_json::to_string_pretty(&hardware_check::run())?);
        return Ok(());
    }
    if let Some(i) = args.iter().position(|a| a == "--capture") {
        let path = args.get(i + 1).ok_or("Informe o caminho PNG")?;
        let preview = preview::Preview::start(1920, 1080, 30);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(35);
        while std::time::Instant::now() < deadline {
            if let Some((w, h, rgb)) = preview.latest() {
                image::save_buffer(path, &rgb, w, h, image::ColorType::Rgb8)?;
                println!("{}: {w}x{h} RGB; {}", path, preview.status());
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        return Err(preview.status().into());
    }
    if args.iter().any(|a| a == "--probe") {
        let usb = uvc::UvcCamera::open();
        let mut hid = hid::PixyHid::connect();
        println!(
            "{}",
            serde_json::to_string_pretty(&controller::read_snapshot(&usb, &mut hid))?
        );
        return Ok(());
    }
    if args.iter().any(|a| a == "--self-test-camera") {
        let camera = uvc::UvcCamera::open().map_err(std::io::Error::other)?;
        let before = camera
            .controls()
            .into_iter()
            .find(|c| c.id == uvc::ControlId::Brightness)
            .and_then(|c| c.current)
            .ok_or("Brilho indisponível")?;
        camera
            .set(uvc::ControlId::Brightness, before)
            .map_err(std::io::Error::other)?;
        let after = camera
            .controls()
            .into_iter()
            .find(|c| c.id == uvc::ControlId::Brightness)
            .and_then(|c| c.current);
        if after != Some(before) {
            return Err("Leitura após escrita divergente".into());
        }
        println!("PASS: USB UVC brightness read/write/read = {before}; configuração preservada");
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 760.0])
            .with_min_inner_size([1100.0, 680.0]),
        vsync: true,
        ..Default::default()
    };
    eframe::run_native(
        "Pixy Control",
        options,
        Box::new(|cc| Ok(Box::new(ui::PixyApp::new(cc)))),
    )?;
    Ok(())
}
