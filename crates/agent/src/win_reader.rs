//! Leitores Windows estritamente **Somente Leitura** (`zengravity-agent::win_reader`).
//!
//! Regras inegociáveis de segurança implementadas aqui:
//! 1. **Apenas `PROCESS_VM_READ | PROCESS_QUERY_INFORMATION` ou `PROCESS_QUERY_LIMITED_INFORMATION`**:
//!    nenhum handle de processo é aberto com permissão de escrita (`PROCESS_VM_WRITE` / `PROCESS_VM_OPERATION`
//!    não são sequer importados).
//! 2. **Um único `PrintWindow` por janela por ciclo (`capture_single_frame`)**:
//!    rejeita janelas minimizadas (`IsIconic`), verifica que o `PID`, o retângulo e a origem da janela
//!    não mudaram durante a captura e normaliza na origem padrão `CLIENT_AT = (3, 26)`.
//! 3. **Zero automação / Zero `SendInput` / Zero leitura de `Settings.ini`**.

use crate::image::FrameImage;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;
use std::time::Instant;
use windows::Win32::Foundation::{CloseHandle, BOOL, FILETIME, HANDLE, HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
    GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
};
use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
};
use windows::Win32::Networking::WinSock::AF_INET;
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows::Win32::System::Memory::{
    VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE_READWRITE, PAGE_GUARD,
    PAGE_NOACCESS, PAGE_READWRITE, PAGE_WRITECOPY,
};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindow, IsWindowVisible,
};
use zengravity_core::clock::MonotonicMs;
use zengravity_core::entities::{EntityKind, NearbyEntity, OBJECT_STRIDE};

/// Classe da janela principal do cliente "The Classic MU Season 2".
pub const DEFAULT_WINDOW_CLASS: &str = "The Classic MU Season 2";

/// Posição da área cliente dentro do quadro padrão 1030×797 (borda 3 px, título 26 px).
pub const CLIENT_AT: (i32, i32) = (3, 26);

/// Recortes visuais calibrados no quadro padrão 1030×797:
pub const REGION_CORNER_MAP_COORDS: [i32; 4] = [0, 30, 300, 24];
pub const REGION_HELPER_WIDE: [i32; 4] = [232, 30, 88, 24];
pub const REGION_ZEN_INVENTORY: [i32; 4] = [786, 609, 78, 14];
pub const REGION_HP_ORB: [i32; 4] = [165, 752, 100, 36];
pub const REGION_RAVEN_SCAN: [i32; 4] = [100, 40, 820, 45];

const PW_RENDERFULLCONTENT: u32 = 2;
const MIB_TCP_STATE_ESTAB: u32 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredWindow {
    pub pid: u32,
    pub hwnd: isize,
    pub title: String,
    pub is_minimized: bool,
    pub process_start_time: Option<u64>,
    pub exe_path: Option<String>,
}

struct EnumCtx {
    target_class: String,
    windows: Vec<DiscoveredWindow>,
}

unsafe extern "system" fn enum_windows_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = unsafe { &mut *(lparam.0 as *mut EnumCtx) };
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return BOOL(1);
    }
    let mut class_buf = [0u16; 256];
    let cn = unsafe { GetClassNameW(hwnd, &mut class_buf) };
    let class_name = if cn > 0 {
        String::from_utf16_lossy(&class_buf[..cn as usize])
    } else {
        String::new()
    };

    let mut title_buf = [0u16; 512];
    let tn = unsafe { GetWindowTextW(hwnd, &mut title_buf) };
    let title = String::from_utf16_lossy(&title_buf[..tn.max(0) as usize]);

    let matches_class_or_title = class_name == ctx.target_class
        || title.contains("TheClassic MU")
        || title.contains("The Classic MU");

    if !matches_class_or_title {
        return BOOL(1);
    }

    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    if pid != 0 {
        let is_minimized = unsafe { IsIconic(hwnd) }.as_bool();
        ctx.windows.push(DiscoveredWindow {
            pid,
            hwnd: hwnd.0 as isize,
            title,
            is_minimized,
            process_start_time: query_process_start_time(pid),
            exe_path: query_process_exe_path(pid),
        });
    }
    BOOL(1)
}

/// Enumera todas as janelas visíveis do cliente MU Online pela classe da janela.
/// Operação 100% passiva: não injeta nada e não altera foco.
pub fn discover_game_windows(window_class: &str) -> Vec<DiscoveredWindow> {
    let mut ctx = EnumCtx {
        target_class: window_class.to_string(),
        windows: Vec::new(),
    };
    unsafe {
        let _ = EnumWindows(
            Some(enum_windows_cb),
            LPARAM(&mut ctx as *mut EnumCtx as isize),
        );
    }
    ctx.windows.sort_by_key(|w| w.pid);
    ctx.windows
}

pub fn query_process_start_time(pid: u32) -> Option<u64> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut create, mut exit, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        let ok = GetProcessTimes(h, &mut create, &mut exit, &mut kernel, &mut user).is_ok();
        let _ = CloseHandle(h);
        ok.then(|| (u64::from(create.dwHighDateTime) << 32) | u64::from(create.dwLowDateTime))
    }
}

pub fn query_process_exe_path(pid: u32) -> Option<String> {
    use windows::core::PWSTR;
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok =
            QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
                .is_ok();
        let _ = CloseHandle(h);
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Verifica na tabela TCP do Windows se o PID do cliente possui conexão TCP estabelecida (`ESTABLISHED`)
/// com um servidor externo (não loopback).
pub fn has_external_tcp_connection(pid: u32, allow_loopback: bool) -> bool {
    let mut size: u32 = 0;
    unsafe {
        let _ = GetExtendedTcpTable(
            None,
            &mut size,
            false,
            AF_INET.0 as u32,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        );
        if size == 0 {
            return false;
        }
        let mut buf = vec![0u8; size as usize + 4096];
        size = buf.len() as u32;
        let rc = GetExtendedTcpTable(
            Some(buf.as_mut_ptr() as *mut _),
            &mut size,
            false,
            AF_INET.0 as u32,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        );
        if rc != 0 {
            return false;
        }
        let table = &*(buf.as_ptr() as *const MIB_TCPTABLE_OWNER_PID);
        let rows = std::slice::from_raw_parts(table.table.as_ptr(), table.dwNumEntries as usize);
        rows.iter().any(|r| {
            if r.dwOwningPid != pid || r.dwState != MIB_TCP_STATE_ESTAB {
                return false;
            }
            let first_octet = (r.dwRemoteAddr & 0xFF) as u8;
            let is_loopback = first_octet == 127 || r.dwRemoteAddr == 0;
            allow_loopback || !is_loopback
        })
    }
}

fn window_pid(hwnd: isize) -> Option<u32> {
    let h = HWND(hwnd as *mut _);
    if !unsafe { IsWindow(h) }.as_bool() {
        return None;
    }
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(h, Some(&mut pid));
    }
    (pid != 0).then_some(pid)
}

fn frame_origin(hwnd: isize) -> Option<(i32, i32)> {
    let mut p = POINT { x: 0, y: 0 };
    unsafe { ClientToScreen(HWND(hwnd as *mut _), &mut p) }
        .ok()
        .ok()?;
    Some((p.x - CLIENT_AT.0, p.y - CLIENT_AT.1))
}

static CAPTURE_LOCK: Mutex<()> = Mutex::new(());

/// Captura **um único quadro** da janela via `PrintWindow(PW_RENDERFULLCONTENT)` sem tirá-la de segundo plano.
/// Rejeita janelas minimizadas ou cujo PID/geometria mudou durante a leitura.
pub fn capture_single_frame(hwnd: isize) -> Option<FrameImage> {
    let _guard = CAPTURE_LOCK.lock().ok()?;
    let pid = window_pid(hwnd)?;
    let origin = frame_origin(hwnd)?;
    let h = HWND(hwnd as *mut _);

    unsafe {
        if IsIconic(h).as_bool() {
            return None;
        }
        let mut rc = RECT::default();
        GetWindowRect(h, &mut rc).ok()?;
        let (w, height) = (rc.right - rc.left, rc.bottom - rc.top);
        if w <= 0 || height <= 0 || w > 8192 || height > 8192 {
            return None;
        }
        let hdc = GetDC(h);
        if hdc.0.is_null() {
            return None;
        }
        let mem = CreateCompatibleDC(hdc);
        if mem.0.is_null() {
            ReleaseDC(h, hdc);
            return None;
        }
        let bmp = CreateCompatibleBitmap(hdc, w, height);
        if bmp.0.is_null() {
            let _ = DeleteDC(mem);
            ReleaseDC(h, hdc);
            return None;
        }
        let old = SelectObject(mem, bmp);
        let ok = PrintWindow(h, mem, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)).as_bool();
        SelectObject(mem, old);

        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -height, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buf = vec![0u8; (w * height * 4) as usize];
        let lines = GetDIBits(
            mem,
            bmp,
            0,
            height as u32,
            Some(buf.as_mut_ptr() as *mut _),
            &mut bi,
            DIB_RGB_COLORS,
        );

        let _ = DeleteObject(bmp);
        let _ = DeleteDC(mem);
        ReleaseDC(h, hdc);

        let mut after = RECT::default();
        if !ok
            || lines != height
            || window_pid(hwnd) != Some(pid)
            || GetWindowRect(h, &mut after).is_err()
            || (after.left, after.top, after.right, after.bottom)
                != (rc.left, rc.top, rc.right, rc.bottom)
            || frame_origin(hwnd) != Some(origin)
        {
            return None;
        }

        let raw = FrameImage {
            w: w as u32,
            h: height as u32,
            bgra: buf,
        };
        let (fx, fy) = origin;
        let (dx, dy) = (rc.left - fx, rc.top - fy);
        if dx == 0 && dy == 0 {
            Some(raw)
        } else {
            let (nw, nh) = ((w + dx).max(1), (height + dy).max(1));
            let mut out = vec![0u8; (nw * nh * 4) as usize];
            for y in 0..height {
                let ty = y + dy;
                if ty < 0 || ty >= nh {
                    continue;
                }
                for x in 0..w {
                    let tx = x + dx;
                    if tx < 0 || tx >= nw {
                        continue;
                    }
                    let si = ((y * w + x) * 4) as usize;
                    let ti = ((ty * nw + tx) * 4) as usize;
                    out[ti..ti + 4].copy_from_slice(&raw.bgra[si..si + 4]);
                }
            }
            Some(FrameImage {
                w: nw as u32,
                h: nh as u32,
                bgra: out,
            })
        }
    }
}

/// Handle RAII de leitura de memória do processo (`PROCESS_VM_READ | PROCESS_QUERY_INFORMATION`).
/// Jamais solicita permissões de escrita.
pub struct ReadOnlyProcess(HANDLE);

impl ReadOnlyProcess {
    pub fn open(pid: u32) -> Option<Self> {
        unsafe {
            OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, false, pid)
                .ok()
                .map(ReadOnlyProcess)
        }
    }

    pub fn read_bytes(&self, addr: u64, buf: &mut [u8]) -> bool {
        let mut bytes_read = 0usize;
        unsafe {
            ReadProcessMemory(
                self.0,
                addr as *const c_void,
                buf.as_mut_ptr() as *mut c_void,
                buf.len(),
                Some(&mut bytes_read),
            )
            .is_ok()
                && bytes_read == buf.len()
        }
    }

    pub fn writable_committed_regions(&self) -> Vec<(u64, usize)> {
        let mut out = Vec::new();
        let mut addr: u64 = 0x10000;
        let max: u64 = 0x7FFF_FFFF_0000;
        while addr < max {
            let mut mbi = MEMORY_BASIC_INFORMATION::default();
            let got = unsafe {
                VirtualQueryEx(
                    self.0,
                    Some(addr as *const c_void),
                    &mut mbi,
                    std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            };
            if got == 0 {
                break;
            }
            let base = mbi.BaseAddress as u64;
            let size = mbi.RegionSize;
            let p = mbi.Protect;
            let writable = (p & (PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READWRITE)).0 != 0;
            let bad = (p & (PAGE_GUARD | PAGE_NOACCESS)).0 != 0;
            if mbi.State == MEM_COMMIT && writable && !bad && size <= 256 * 1024 * 1024 {
                out.push((base, size));
            }
            let next = base.saturating_add(size as u64);
            if next <= addr {
                break;
            }
            addr = next;
        }
        out
    }
}

impl Drop for ReadOnlyProcess {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

pub fn read_u32(pid: u32, addr: u64) -> Option<u32> {
    let p = ReadOnlyProcess::open(pid)?;
    let mut b = [0u8; 4];
    p.read_bytes(addr, &mut b).then(|| u32::from_le_bytes(b))
}

pub fn scan_u32(pid: u32, value: u32) -> Vec<u64> {
    let Some(p) = ReadOnlyProcess::open(pid) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    const CHUNK: usize = 1 << 20;
    let mut buf = vec![0u8; CHUNK];
    for (base, size) in p.writable_committed_regions() {
        let mut off = 0usize;
        while off < size {
            let n = CHUNK.min(size - off);
            if p.read_bytes(base + off as u64, &mut buf[..n]) {
                out.extend(zengravity_core::ramlock::find_u32(
                    &buf[..n],
                    base + off as u64,
                    value,
                ));
            }
            off += n;
        }
        if out.len() > 100_000 {
            break;
        }
    }
    out
}

pub fn scan_u16(pid: u32, value: u16, limit: usize) -> Vec<u64> {
    let Some(p) = ReadOnlyProcess::open(pid) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    const CHUNK: usize = 1 << 20;
    let mut buf = vec![0u8; CHUNK];
    let want = value.to_le_bytes();
    for (base, size) in p.writable_committed_regions() {
        let mut off = 0usize;
        while off < size {
            let n = CHUNK.min(size - off);
            let at = base + off as u64;
            if p.read_bytes(at, &mut buf[..n]) {
                for i in (0..n.saturating_sub(1)).step_by(2) {
                    if buf[i] == want[0] && buf[i + 1] == want[1] {
                        out.push(at + i as u64);
                    }
                }
            }
            off += n;
            if out.len() > limit {
                return out;
            }
        }
    }
    out
}

pub fn read_u16_many(pid: u32, addrs: &[u64]) -> Vec<Option<u16>> {
    let mut out = vec![None; addrs.len()];
    let Some(p) = ReadOnlyProcess::open(pid) else {
        return out;
    };
    let mut order: Vec<usize> = (0..addrs.len()).collect();
    order.sort_by_key(|&i| addrs[i]);
    const SPAN: u64 = 64 * 1024;
    let mut buf = vec![0u8; SPAN as usize + 2];
    let mut k = 0;
    while k < order.len() {
        let start = addrs[order[k]];
        let mut j = k;
        while j < order.len() && addrs[order[j]] + 2 <= start + SPAN {
            j += 1;
        }
        let end = addrs[order[j - 1]] + 2;
        let len = (end - start) as usize;
        if p.read_bytes(start, &mut buf[..len]) {
            for &i in &order[k..j] {
                let o = (addrs[i] - start) as usize;
                out[i] = Some(u16::from_le_bytes([buf[o], buf[o + 1]]));
            }
        } else {
            for &i in &order[k..j] {
                let mut b = [0u8; 2];
                if p.read_bytes(addrs[i], &mut b) {
                    out[i] = Some(u16::from_le_bytes(b));
                }
            }
        }
        k = j;
    }
    out
}

/// Cache do endereço-âncora da entrada do próprio personagem na tabela de objetos de cada PID.
static HERO_ANCHOR_CACHE: Mutex<Option<HashMap<u32, u64>>> = Mutex::new(None);
static HERO_SCAN_COOLDOWN: Mutex<Option<HashMap<u32, Instant>>> = Mutex::new(None);
const OBJECT_SPAN_ENTRIES: usize = 700;

fn verify_hero_anchor(
    p: &ReadOnlyProcess,
    addr: u64,
    char_name: &str,
    pos: (i32, i32),
    now_mono_ms: MonotonicMs,
) -> bool {
    let mut entry = vec![0u8; OBJECT_STRIDE];
    if !p.read_bytes(addr, &mut entry) {
        return false;
    }
    match NearbyEntity::parse_raw_entry(&entry, char_name, now_mono_ms) {
        Some(ent) => {
            ent.kind == EntityKind::Player
                && ent.name == char_name
                && (ent.x - pos.0).abs() <= 2
                && (ent.y - pos.1).abs() <= 2
        }
        None => false,
    }
}

fn find_hero_anchor(
    p: &ReadOnlyProcess,
    char_name: &str,
    pos: (i32, i32),
    now_mono_ms: MonotonicMs,
) -> Option<u64> {
    let pat: Vec<u8> = char_name
        .as_bytes()
        .iter()
        .copied()
        .chain(Some(0))
        .collect();
    let mut buf = vec![0u8; 1 << 20];
    for (base, size) in p.writable_committed_regions() {
        let mut off = 0usize;
        while off < size {
            let n = buf.len().min(size - off);
            if p.read_bytes(base + off as u64, &mut buf[..n]) {
                let mut i = 0;
                while i + pat.len() <= n {
                    if buf[i..i + pat.len()] == pat[..] {
                        let candidate = base + (off + i) as u64;
                        if verify_hero_anchor(p, candidate, char_name, pos, now_mono_ms) {
                            return Some(candidate);
                        }
                    }
                    i += 1;
                }
            }
            off += n.saturating_sub(pat.len()).max(1);
        }
    }
    None
}

/// Lê a lista de entidades vivas (jogadores, monstros, NPCs, Golden Monsters) próximas ao personagem `char_name`.
pub fn read_nearby_entities(
    pid: u32,
    char_name: &str,
    pos: (i32, i32),
    now_mono_ms: MonotonicMs,
) -> Option<Vec<NearbyEntity>> {
    let p = ReadOnlyProcess::open(pid)?;
    let cached = HERO_ANCHOR_CACHE
        .lock()
        .ok()?
        .get_or_insert_with(HashMap::new)
        .get(&pid)
        .copied();
    let hero_addr = match cached.filter(|&a| verify_hero_anchor(&p, a, char_name, pos, now_mono_ms))
    {
        Some(a) => a,
        None => {
            {
                let mut guard = HERO_SCAN_COOLDOWN.lock().ok()?;
                let map = guard.get_or_insert_with(HashMap::new);
                if map.get(&pid).is_some_and(|t| t.elapsed().as_secs() < 45) {
                    return None;
                }
                map.insert(pid, Instant::now());
            }
            let found = find_hero_anchor(&p, char_name, pos, now_mono_ms)?;
            if let Ok(mut guard) = HERO_SCAN_COOLDOWN.lock() {
                guard.get_or_insert_with(HashMap::new).remove(&pid);
            }
            HERO_ANCHOR_CACHE
                .lock()
                .ok()?
                .get_or_insert_with(HashMap::new)
                .insert(pid, found);
            found
        }
    };

    let start = hero_addr.saturating_sub((OBJECT_SPAN_ENTRIES * OBJECT_STRIDE) as u64);
    let len = 2 * OBJECT_SPAN_ENTRIES * OBJECT_STRIDE;
    let mut buf = vec![0u8; len];
    for off in (0..len).step_by(4096) {
        let n = 4096.min(len - off);
        let _ = p.read_bytes(start + off as u64, &mut buf[off..off + n]);
    }
    let anchor = (hero_addr - start) as usize;
    let mut entities = NearbyEntity::parse_raw_block(&buf, anchor, char_name, now_mono_ms);
    entities.retain(|e| !(e.kind == EntityKind::Player && e.name == char_name));
    Some(entities)
}

/// Quando a janela está minimizada, permite ler a coordenada `(x, y)` do próprio personagem
/// diretamente da entrada já ancorada na lista de objetos da memória (sem `PrintWindow`).
pub fn read_minimized_hero_position(
    pid: u32,
    char_name: &str,
    now_mono_ms: MonotonicMs,
) -> Option<(i32, i32)> {
    let addr = HERO_ANCHOR_CACHE
        .lock()
        .ok()?
        .as_ref()?
        .get(&pid)
        .copied()?;
    let p = ReadOnlyProcess::open(pid)?;
    let mut entry = vec![0u8; OBJECT_STRIDE];
    if !p.read_bytes(addr, &mut entry) {
        return None;
    }
    let ent = NearbyEntity::parse_raw_entry(&entry, char_name, now_mono_ms)?;
    (ent.kind == EntityKind::Player
        && ent.name == char_name
        && (0..=255).contains(&ent.x)
        && (0..=255).contains(&ent.y))
    .then_some((ent.x, ent.y))
}
