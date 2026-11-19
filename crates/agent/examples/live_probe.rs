use zengravity_agent::live_collector::{ReadOnlyLiveCollector, Win32ReadOnlyAccess};
use zengravity_agent::win_reader::{discover_game_windows, DEFAULT_WINDOW_CLASS};
use zengravity_protocol::AgentToCentral;

fn main() {
    let windows = discover_game_windows(DEFAULT_WINDOW_CLASS);
    println!("=== ZENGRAVITY FASE 2 — SONDA SOMENTE LEITURA (MainMU.exe) ===");
    println!(
        "Janelas '{}' encontradas: {}",
        DEFAULT_WINDOW_CLASS,
        windows.len()
    );
    for w in &windows {
        println!(
            "  • PID={} | HWND={:#x} | Minimizada={} | StartTime={:?} | Exe={:?} | Titulo='{}'",
            w.pid, w.hwnd, w.is_minimized, w.process_start_time, w.exe_path, w.title
        );
    }

    let access = Win32ReadOnlyAccess::default();
    let mut collector = ReadOnlyLiveCollector::new("PC-LOCAL-SONDA", None);
    if let Some(dir) = collector.terrain_loader.data_dir() {
        println!("Pasta Data do cliente descoberta: {}", dir.display());
    }
    let msg = collector.collect_cycle(&access, 1000, 1_795_000_000);
    if let AgentToCentral::Report { chars, .. } = msg {
        for c in chars {
            println!(
                "  -> Char={:?} (Lv.{:?} / Resets {:?}) | TCP={} | Pos={:?} ({:?}) | Helper={:?} | HP={:?} | Zen={:?} | Detalhe='{}'",
                c.title.name,
                c.title.level,
                c.title.resets,
                c.tcp_connected,
                c.position.value,
                c.position.source,
                c.helper_active.value,
                c.hp.value,
                c.zen.value,
                c.recovery_detail.unwrap_or_default()
            );
            if let Some(ents) = &c.nearby_entities.value {
                println!(
                    "     Entidades próximas na RAM (0x598): {} encontradas",
                    ents.len()
                );
                for e in ents.iter().take(8) {
                    println!(
                        "       - [{:?}] {} (classe {}) em ({}, {})",
                        e.kind, e.name, e.class_id, e.x, e.y
                    );
                }
            }
        }
    }
}
