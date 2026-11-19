//! Observador passivo dos atalhos físicos `F1` e `F2` no jogo (`zengravity-agent::hotkey_watcher`).
//!
//! Regras inegociáveis de segurança (ADR 0017):
//! 1. **Somente leitura / observação**: o gancho (`WH_KEYBOARD_LL`) **jamais** bloqueia a tecla;
//!    sempre chama `CallNextHookEx` para que o jogo receba a tecla normalmente.
//! 2. **Apenas eventos físicos reais**: eventos injetados por software (`LLKHF_INJECTED`)
//!    ou acompanhados de modificadores (`Ctrl`, `Alt`, `Shift`) são ignorados.
//! 3. **Sem auto-repetição**: segurar `F1` ou `F2` dispara apenas **uma** captura até que a tecla
//!    seja solta (`WM_KEYUP`).
//! 4. **Somente com o jogo em primeiro plano**: se a janela em foco (`GetForegroundWindow`)
//!    não for do cliente MU Online, o evento é ignorado.
//! 5. **Captura no instante exato**: as coordenadas do canto da janela são lidas do quadro
//!    capturado no momento da tecla (não usa posição velha ou futura).

use crate::win_reader::{capture_single_frame, DEFAULT_WINDOW_CLASS, REGION_CORNER_MAP_COORDS};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use zengravity_core::game_title::TitleInfo;
use zengravity_core::glyphs::{binarize_white, map_name, read_coords, COORDS_WHITE};
use zengravity_core::map::WorldId;
use zengravity_core::spots::SpotCoord;

/// Tipo do atalho físico pressionado pelo operador com o jogo em foco.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpotHotkeyKind {
    /// `F1` (`VK_F1 = 0x70`): define o tile atual como Spot Principal (`home_spot`) do personagem
    /// em foco e também o inclui no catálogo do mapa.
    AssignHomeSpotF1,
    /// `F2` (`VK_F2 = 0x71`): apenas cataloga o tile atual no mapa, preservando o `home_spot` anterior.
    CatalogSpotF2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedHotkeyEvent {
    pub kind: SpotHotkeyKind,
    pub hwnd: isize,
    pub char_name: Option<String>,
    pub captured_spot: Option<SpotCoord>,
}

/// Função pura e testável de decodificação da tecla física.
/// Recusa qualquer tecla injetada (`injected == true`) ou com modificadores (`Ctrl`/`Alt`/`Shift`).
pub fn decode_physical_spot_hotkey(
    vk_code: u32,
    injected: bool,
    has_modifiers: bool,
) -> Option<SpotHotkeyKind> {
    if injected || has_modifiers {
        return None;
    }
    match vk_code {
        0x70 => Some(SpotHotkeyKind::AssignHomeSpotF1),
        0x71 => Some(SpotHotkeyKind::CatalogSpotF2),
        _ => None,
    }
}

/// Máquina pura de anti-repetição para teclas mantidas pressionadas (`WM_KEYDOWN` repetido antes de `WM_KEYUP`).
#[derive(Debug, Default)]
pub struct KeyHoldTracker {
    held_mask: AtomicU32,
}

impl KeyHoldTracker {
    pub const fn new() -> Self {
        Self {
            held_mask: AtomicU32::new(0),
        }
    }

    fn bit_for(kind: SpotHotkeyKind) -> u32 {
        match kind {
            SpotHotkeyKind::AssignHomeSpotF1 => 0b01,
            SpotHotkeyKind::CatalogSpotF2 => 0b10,
        }
    }

    /// Retorna `true` apenas na borda de subida (primeiro `KEYDOWN` físico).
    pub fn on_key_down(&self, kind: SpotHotkeyKind) -> bool {
        let bit = Self::bit_for(kind);
        (self.held_mask.fetch_or(bit, Ordering::SeqCst) & bit) == 0
    }

    pub fn on_key_up(&self, vk_code: u32) {
        let bit = match vk_code {
            0x70 => 0b01,
            0x71 => 0b10,
            _ => return,
        };
        self.held_mask.fetch_and(!bit, Ordering::SeqCst);
    }
}

static GLOBAL_HOLD_TRACKER: KeyHoldTracker = KeyHoldTracker::new();
static HOTKEY_TX: OnceLock<Mutex<Sender<(SpotHotkeyKind, isize, String)>>> = OnceLock::new();

#[cfg(windows)]
fn is_key_down(vk: u16) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    unsafe { (GetAsyncKeyState(vk as i32) as u16 & 0x8000) != 0 }
}

#[cfg(windows)]
unsafe extern "system" fn low_level_keyboard_proc(
    code: i32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_CONTROL, VK_MENU, VK_SHIFT};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetClassNameW, GetForegroundWindow, GetWindowTextW, KBDLLHOOKSTRUCT,
        LLKHF_INJECTED, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYUP,
    };

    if code >= 0 {
        let msg = wparam.0 as u32;
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let injected = (kb.flags.0 & (LLKHF_INJECTED.0 | 0x02)) != 0;

        if matches!(msg, WM_KEYUP | WM_SYSKEYUP) && !injected {
            GLOBAL_HOLD_TRACKER.on_key_up(kb.vkCode);
        } else if msg == WM_KEYDOWN {
            let modifiers =
                is_key_down(VK_CONTROL.0) || is_key_down(VK_MENU.0) || is_key_down(VK_SHIFT.0);
            if let Some(kind) = decode_physical_spot_hotkey(kb.vkCode, injected, modifiers) {
                if GLOBAL_HOLD_TRACKER.on_key_down(kind) {
                    let fg = unsafe { GetForegroundWindow() };
                    if !fg.0.is_null() {
                        let mut cbuf = [0u16; 128];
                        let cn = unsafe { GetClassNameW(fg, &mut cbuf) };
                        let class_name = String::from_utf16_lossy(&cbuf[..cn.max(0) as usize]);

                        let mut tbuf = [0u16; 512];
                        let tn = unsafe { GetWindowTextW(fg, &mut tbuf) };
                        let title = String::from_utf16_lossy(&tbuf[..tn.max(0) as usize]);

                        let is_mu_window = class_name == DEFAULT_WINDOW_CLASS
                            || title.contains("TheClassic MU")
                            || title.contains("The Classic MU");

                        if is_mu_window {
                            if let Some(tx_mutex) = HOTKEY_TX.get() {
                                if let Ok(tx) = tx_mutex.lock() {
                                    let _ = tx.send((kind, fg.0 as isize, title));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Inicia a thread de observação passiva dos atalhos `F1` e `F2` no Windows e retorna um `Receiver`
/// com os eventos já enriquecidos pelo recorte do canto da janela capturado no instante do atalho.
pub fn spawn_passive_hotkey_watcher() -> Receiver<CapturedHotkeyEvent> {
    let (raw_tx, raw_rx) = channel::<(SpotHotkeyKind, isize, String)>();
    let (out_tx, out_rx) = channel::<CapturedHotkeyEvent>();

    let _ = HOTKEY_TX.set(Mutex::new(raw_tx));

    // Thread que realiza a captura imediata do canto da janela ao receber o sinal do gancho
    std::thread::spawn(move || {
        while let Ok((kind, hwnd, title)) = raw_rx.recv() {
            let char_name = TitleInfo::parse(&title).name;
            let captured_spot = capture_single_frame(hwnd)
                .and_then(|frame| frame.crop(REGION_CORNER_MAP_COORDS))
                .and_then(|crop| {
                    let bits =
                        binarize_white(&crop.bgra, crop.w as usize, crop.h as usize, COORDS_WHITE);
                    let m_name = map_name(&bits)?;
                    let (cx, cy) = read_coords(&bits)?;
                    let world = WorldId::from_name(m_name)?;
                    Some(SpotCoord::new(world, cx as i32, cy as i32))
                });

            let _ = out_tx.send(CapturedHotkeyEvent {
                kind,
                hwnd,
                char_name,
                captured_spot,
            });
        }
    });

    #[cfg(windows)]
    std::thread::spawn(|| unsafe {
        use windows::Win32::Foundation::HINSTANCE;
        use windows::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows::Win32::UI::WindowsAndMessaging::{
            GetMessageW, SetWindowsHookExW, MSG, WH_KEYBOARD_LL,
        };

        let Ok(hmod) = GetModuleHandleW(None) else {
            return;
        };
        if SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(low_level_keyboard_proc),
            HINSTANCE::from(hmod),
            0,
        )
        .is_ok()
        {
            tracing::info!(
                "Observador passivo F1/F2 ativo (somente com janela do MU Online em primeiro plano)"
            );
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
        }
    });

    out_rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_physical_unmodified_f1_and_f2_and_blocks_key_repeat() {
        assert_eq!(
            decode_physical_spot_hotkey(0x70, false, false),
            Some(SpotHotkeyKind::AssignHomeSpotF1)
        );
        assert_eq!(
            decode_physical_spot_hotkey(0x71, false, false),
            Some(SpotHotkeyKind::CatalogSpotF2)
        );
        // Injetado ou com Ctrl/Alt/Shift é recusado
        assert_eq!(decode_physical_spot_hotkey(0x70, true, false), None);
        assert_eq!(decode_physical_spot_hotkey(0x71, false, true), None);
        // Teclas antigas (Ctrl+D / Ctrl+S) jamais disparam
        assert_eq!(decode_physical_spot_hotkey(0x44, false, false), None);

        let tracker = KeyHoldTracker::new();
        assert!(tracker.on_key_down(SpotHotkeyKind::AssignHomeSpotF1));
        assert!(
            !tracker.on_key_down(SpotHotkeyKind::AssignHomeSpotF1),
            "segurar F1 não deve disparar repetição"
        );
        tracker.on_key_up(0x70);
        assert!(
            tracker.on_key_down(SpotHotkeyKind::AssignHomeSpotF1),
            "após soltar F1 deve aceitar novo toque"
        );
    }
}
