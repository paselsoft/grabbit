// SPDX-License-Identifier: GPL-3.0-or-later
//! Icona di Grabbit nella barra dei menu del Mac, con il suo menu, e numero sull'icona del Dock.
//!
//! Il disegno delle icone e la scelta di dove va un link copiato sono funzioni normali (provate
//! anche su Linux); la parte che parla con macOS sta in `mac`, che altrove non fa niente.
// fuori dal Mac le icone e il menu servono solo ai test
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use crate::tr;

/// Lato dell'icona in pixel: macOS la mostra alta 18 punti, quindi 36 px restano nitidi sui
/// Retina e vanno bene anche sugli schermi normali.
pub const ICON_PX: u32 = 36;

/// Come appare l'icona nella barra dei menu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Look {
    Idle,
    /// avanzamento 0–1 arrotondato a passi di 1/12 (la barretta ha 24 px)
    Progress(u8),
    Done,
    Failed,
}

impl Look {
    pub fn progress(fraction: f32) -> Self {
        Look::Progress((fraction.clamp(0.0, 1.0) * 12.0).round() as u8)
    }
}

/// Disegna l'icona: nero con trasparenza (macOS la colora da sé, chiara o scura).
pub fn draw(look: Look) -> Vec<u8> {
    let n = ICON_PX as usize;
    let mut alpha = vec![0f32; n * n];
    let mut seg = |a: (f32, f32), b: (f32, f32), w: f32| stroke(&mut alpha, n, a, b, w);
    let arrow = |seg: &mut dyn FnMut((f32, f32), (f32, f32), f32)| {
        seg((18.0, 5.0), (18.0, 21.0), 3.4);
        seg((11.0, 14.5), (18.0, 21.5), 3.4);
        seg((25.0, 14.5), (18.0, 21.5), 3.4);
    };
    let tray = |seg: &mut dyn FnMut((f32, f32), (f32, f32), f32)| {
        seg((6.0, 25.0), (6.0, 29.5), 3.0);
        seg((6.0, 29.5), (30.0, 29.5), 3.0);
        seg((30.0, 29.5), (30.0, 25.0), 3.0);
    };
    match look {
        Look::Idle => {
            arrow(&mut seg);
            tray(&mut seg);
        }
        Look::Failed => {
            arrow(&mut seg);
            tray(&mut seg);
            disc(&mut alpha, n, (29.5, 6.5), 5.5);
        }
        Look::Done => {
            seg((7.0, 19.0), (14.0, 26.0), 3.6);
            seg((14.0, 26.0), (29.0, 10.0), 3.6);
        }
        Look::Progress(steps) => {
            arrow(&mut seg);
            // bordo della barretta e riempimento
            seg((7.0, 26.5), (29.0, 26.5), 1.4);
            seg((7.0, 32.5), (29.0, 32.5), 1.4);
            seg((6.3, 26.5), (6.3, 32.5), 1.4);
            seg((29.7, 26.5), (29.7, 32.5), 1.4);
            let end = 7.0 + 22.0 * (steps.min(12) as f32 / 12.0);
            for y in 27..33 {
                for x in 7..(end.round() as usize) {
                    alpha[y * n + x] = 1.0;
                }
            }
        }
    }
    let mut rgba = Vec::with_capacity(n * n * 4);
    for a in alpha {
        rgba.extend_from_slice(&[0, 0, 0, (a.clamp(0.0, 1.0) * 255.0).round() as u8]);
    }
    rgba
}

/// Tratto con le punte arrotondate, sfumato sui bordi.
fn stroke(alpha: &mut [f32], n: usize, a: (f32, f32), b: (f32, f32), w: f32) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = (dx * dx + dy * dy).max(1e-6);
    for y in 0..n {
        for x in 0..n {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let t = (((px - a.0) * dx + (py - a.1) * dy) / len2).clamp(0.0, 1.0);
            let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
            let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
            let v = (w / 2.0 + 0.5 - d).clamp(0.0, 1.0);
            let p = &mut alpha[y * n + x];
            *p = p.max(v);
        }
    }
}

fn disc(alpha: &mut [f32], n: usize, c: (f32, f32), r: f32) {
    for y in 0..n {
        for x in 0..n {
            let d = ((x as f32 + 0.5 - c.0).powi(2) + (y as f32 + 0.5 - c.1).powi(2)).sqrt();
            let v = (r + 0.5 - d).clamp(0.0, 1.0);
            let p = &mut alpha[y * n + x];
            *p = p.max(v);
        }
    }
}

/// Dove va un link copiato quando si sceglie «Scarica il link copiato».
#[derive(Debug, PartialEq)]
pub enum LinkKind {
    /// magnet o .torrent: all'app torrent
    Torrent,
    /// un file da scaricare così com'è (.iso, .zip, .dmg…): scheda «File»
    File,
    /// tutto il resto: video o musica con yt-dlp
    Media,
}

/// Riconosce un link copiato; None se non è un link.
pub fn link_kind(text: &str) -> Option<(LinkKind, String)> {
    let t = text.trim();
    if t.is_empty() || t.contains(char::is_whitespace) {
        return None;
    }
    if crate::torrent::is_torrent_source(t) {
        return Some((LinkKind::Torrent, t.to_string()));
    }
    if !(t.starts_with("http://") || t.starts_with("https://")) {
        return None;
    }
    const FILES: [&str; 18] = [
        ".iso",
        ".img",
        ".zip",
        ".7z",
        ".rar",
        ".gz",
        ".xz",
        ".bz2",
        ".tgz",
        ".dmg",
        ".pkg",
        ".exe",
        ".msi",
        ".deb",
        ".rpm",
        ".appimage",
        ".pdf",
        ".epub",
    ];
    let path = t
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let kind = if FILES.iter().any(|e| path.ends_with(e)) {
        LinkKind::File
    } else {
        LinkKind::Media
    };
    Some((kind, t.to_string()))
}

/// Cosa mostra la barra dei menu (lo prepara la finestra a ogni giro).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// download in corso (video e musica, file, torrent)
    pub active: usize,
    /// avanzamento medio di quelli in corso
    pub fraction: Option<f32>,
    /// velocità totale in byte al secondo, quando si conosce
    pub speed: f64,
    /// righe «nome · percentuale»
    pub rows: Vec<(String, String)>,
    pub limit_kbps: u32,
    /// c'è almeno un file o un torrent da mettere in pausa / da riprendere
    pub can_pause: bool,
    pub can_resume: bool,
    pub look_done: bool,
    pub look_failed: bool,
}

impl Status {
    pub fn look(&self) -> Look {
        match self.fraction {
            Some(f) if self.active > 0 => Look::progress(f),
            _ if self.look_failed => Look::Failed,
            _ if self.look_done => Look::Done,
            _ => Look::Idle,
        }
    }

    /// Testo accanto all'icona: la percentuale mentre si scarica.
    pub fn title(&self) -> Option<String> {
        match self.fraction {
            Some(f) if self.active > 0 => Some(format!("{:.0}%", f.clamp(0.0, 1.0) * 100.0)),
            _ => None,
        }
    }

    /// Prima riga del menu.
    pub fn header(&self) -> String {
        let version = env!("CARGO_PKG_VERSION");
        let mut s = match self.active {
            0 => tr!(
                format!("Grabbit {version}, nessun download in corso"),
                format!("Grabbit {version}, no downloads running")
            ),
            1 => tr!(
                format!("Grabbit {version}, 1 download in corso"),
                format!("Grabbit {version}, 1 download running")
            ),
            n => tr!(
                format!("Grabbit {version}, {n} download in corso"),
                format!("Grabbit {version}, {n} downloads running")
            ),
        };
        if self.speed > 0.0 {
            s.push_str(&format!(", {}", crate::dm::fmt_speed(self.speed)));
        }
        s
    }
}

/// Le scelte del limite di velocità nel sottomenu (KB/s, 0 = nessuno).
pub const LIMITS: [u32; 5] = [0, 256, 1024, 5 * 1024, 20 * 1024];

pub fn limit_label(kbps: u32) -> String {
    match kbps {
        0 => tr!("Nessuno", "None").to_string(),
        k if k >= 1024 && k % 1024 == 0 => format!("{} MB/s", k / 1024),
        k => format!("{k} KB/s"),
    }
}

/// Una scelta fatta nel menu.
#[derive(Debug, PartialEq)]
pub enum Action {
    DownloadClipboard,
    PauseAll,
    ResumeAll,
    SpeedLimit(u32),
    OpenApp,
    OpenFolder,
    Quit,
}

impl Action {
    fn id(&self) -> String {
        match self {
            Action::DownloadClipboard => "clip".into(),
            Action::PauseAll => "pause".into(),
            Action::ResumeAll => "resume".into(),
            Action::SpeedLimit(k) => format!("limit:{k}"),
            Action::OpenApp => "open".into(),
            Action::OpenFolder => "folder".into(),
            Action::Quit => "quit".into(),
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "clip" => Action::DownloadClipboard,
            "pause" => Action::PauseAll,
            "resume" => Action::ResumeAll,
            "open" | "row" => Action::OpenApp,
            "folder" => Action::OpenFolder,
            "quit" => Action::Quit,
            other => Action::SpeedLimit(other.strip_prefix("limit:")?.parse().ok()?),
        })
    }
}

/// Passi della barra sull'icona del Dock (si ridisegna solo quando cambia il passo).
pub const DOCK_STEPS: f32 = 24.0;

/// L'icona di Grabbit con la barra di avanzamento in basso, come PNG per il Dock.
pub fn dock_icon_png(fraction: f32) -> Option<Vec<u8>> {
    const SIZE: u32 = 256;
    let base = image::load_from_memory(include_bytes!("../assets/icon.png")).ok()?;
    let mut img = image::imageops::resize(
        &base.to_rgba8(),
        SIZE,
        SIZE,
        image::imageops::FilterType::Triangle,
    );
    // barra con gli angoli tondi: fondo scuro trasparente, riempimento bianco
    let (x0, x1, y0, y1, r) = (40.0_f32, 216.0_f32, 216.0_f32, 238.0_f32, 11.0_f32);
    let fill_end = x0 + (x1 - x0) * fraction.clamp(0.0, 1.0);
    for y in (y0 as u32)..(y1 as u32) {
        for x in (x0 as u32)..(x1 as u32) {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            // distanza dal rettangolo arrotondato: dentro se <= 0
            let qx = (px - (x0 + x1) / 2.0).abs() - ((x1 - x0) / 2.0 - r);
            let qy = (py - (y0 + y1) / 2.0).abs() - ((y1 - y0) / 2.0 - r);
            let d = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() - r;
            let cover = (0.5 - d).clamp(0.0, 1.0);
            if cover <= 0.0 {
                continue;
            }
            let (color, a) = if px <= fill_end {
                ([255.0, 255.0, 255.0], 1.0)
            } else {
                ([26.0, 15.0, 5.0], 0.6)
            };
            let a = a * cover;
            let p = img.get_pixel_mut(x, y);
            for (ch, col) in p.0.iter_mut().zip(color) {
                *ch = (*ch as f32 * (1.0 - a) + col * a).round() as u8;
            }
            p.0[3] = (p.0[3] as f32 + (255.0 - p.0[3] as f32) * a).round() as u8;
        }
    }
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

pub use imp::MenuBar;

#[cfg(target_os = "macos")]
mod imp {
    use super::{draw, limit_label, Action, Look, Status, ICON_PX, LIMITS};
    use crate::tr;
    use std::sync::mpsc::{channel, Receiver};
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

    /// Icona nella barra dei menu e numero sul Dock.
    pub struct MenuBar {
        tray: Option<TrayIcon>,
        rx: Receiver<String>,
        shown: Option<Status>,
        look: Option<Look>,
        badge: Option<String>,
        /// passo della barra sull'icona del Dock mostrato (None = icona normale)
        dock_step: Option<u8>,
        dock_menu_ready: bool,
        failed: bool,
    }

    impl MenuBar {
        /// `wake` ridisegna la finestra quando arriva una scelta dal menu.
        pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
            let (tx, rx) = channel();
            let tx = std::sync::Mutex::new(tx);
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                if let Ok(tx) = tx.lock() {
                    let _ = tx.send(e.id.0);
                }
                wake();
            }));
            Self {
                tray: None,
                rx,
                shown: None,
                look: None,
                badge: None,
                dock_step: None,
                dock_menu_ready: false,
                failed: false,
            }
        }

        pub fn actions(&mut self) -> Vec<Action> {
            self.rx
                .try_iter()
                .filter_map(|id| Action::from_id(&id))
                .collect()
        }

        /// Aggiorna icona, percentuale, menu e numero sul Dock (solo ciò che è cambiato).
        pub fn update(&mut self, st: &Status) {
            if self.failed {
                return;
            }
            let look = st.look();
            if self.tray.is_none() {
                let built = TrayIconBuilder::new()
                    .with_icon(icon(look))
                    .with_icon_as_template(true)
                    .with_tooltip("Grabbit")
                    .with_menu(Box::new(menu(st)))
                    .build();
                match built {
                    Ok(t) => self.tray = Some(t),
                    Err(e) => {
                        eprintln!("barra dei menu non disponibile: {e}");
                        self.failed = true;
                        return;
                    }
                }
                self.look = Some(look);
                self.shown = Some(st.clone());
            }
            let Some(tray) = &self.tray else { return };
            if self.look != Some(look) {
                let _ = tray.set_icon_with_as_template(Some(icon(look)), true);
                self.look = Some(look);
            }
            if self.shown.as_ref() != Some(st) {
                tray.set_title(st.title());
                tray.set_menu(Some(Box::new(menu(st))));
                self.shown = Some(st.clone());
            }
            let badge = (st.active > 0).then(|| st.active.to_string());
            if badge != self.badge {
                dock_badge(badge.as_deref());
                self.badge = badge;
            }
            // barra di avanzamento sull'icona del Dock
            let step = st
                .fraction
                .filter(|_| st.active > 0)
                .map(|f| (f.clamp(0.0, 1.0) * super::DOCK_STEPS).round() as u8);
            if step != self.dock_step {
                dock_progress(step.map(|s| s as f32 / super::DOCK_STEPS));
                self.dock_step = step;
            }
            // menu del tasto destro sul Dock: le stesse voci della barra dei menu
            if !self.dock_menu_ready {
                self.dock_menu_ready = dock::install();
            }
            if dock::wants_update(st) {
                dock::set_menu(menu(st));
                dock::shown(st);
            }
        }
    }

    fn icon(look: Look) -> Icon {
        Icon::from_rgba(draw(look), ICON_PX, ICON_PX).expect("icona della barra dei menu")
    }

    fn menu(st: &Status) -> Menu {
        let m = Menu::new();
        let _ = m.append(&MenuItem::with_id("header", st.header(), false, None));
        for (name, pct) in st.rows.iter().take(6) {
            let _ = m.append(&MenuItem::with_id(
                "row",
                format!("{name}    {pct}"),
                true,
                None,
            ));
        }
        let _ = m.append(&PredefinedMenuItem::separator());
        let item =
            |a: Action, text: &str, enabled: bool| MenuItem::with_id(a.id(), text, enabled, None);
        let _ = m.append(&item(
            Action::DownloadClipboard,
            tr!("Scarica il link copiato", "Download the copied link"),
            true,
        ));
        if st.can_resume && !st.can_pause {
            let _ = m.append(&item(
                Action::ResumeAll,
                tr!("Riprendi file e torrent", "Resume files and torrents"),
                true,
            ));
        } else {
            let _ = m.append(&item(
                Action::PauseAll,
                tr!("Metti in pausa file e torrent", "Pause files and torrents"),
                st.can_pause,
            ));
        }
        let limits = Submenu::new(tr!("Limite di velocità", "Speed limit"), true);
        for k in LIMITS {
            let _ = limits.append(&CheckMenuItem::with_id(
                Action::SpeedLimit(k).id(),
                limit_label(k),
                true,
                st.limit_kbps == k,
                None,
            ));
        }
        // un limite scelto nella finestra che non è tra quelli del menu
        if !LIMITS.contains(&st.limit_kbps) {
            let _ = limits.append(&CheckMenuItem::with_id(
                "custom",
                limit_label(st.limit_kbps),
                false,
                true,
                None,
            ));
        }
        let _ = m.append(&limits);
        let _ = m.append(&PredefinedMenuItem::separator());
        let _ = m.append(&item(
            Action::OpenApp,
            tr!("Apri Grabbit", "Open Grabbit"),
            true,
        ));
        let _ = m.append(&item(
            Action::OpenFolder,
            tr!("Apri la cartella dei download", "Open the downloads folder"),
            true,
        ));
        let _ = m.append(&PredefinedMenuItem::separator());
        let _ = m.append(&item(
            Action::Quit,
            tr!("Esci da Grabbit", "Quit Grabbit"),
            true,
        ));
        m
    }

    /// Icona del Dock con la barra di avanzamento (None rimette l'icona normale).
    /// `setApplicationIconImage` c'è da macOS 10.0.
    fn dock_progress(fraction: Option<f32>) {
        use objc2::AllocAnyThread;
        use objc2_app_kit::{NSApplication, NSImage};
        use objc2_foundation::{MainThreadMarker, NSData};
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        let image = fraction
            .and_then(super::dock_icon_png)
            .and_then(|png| NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(&png)));
        // SAFETY: immagine valida (o nessuna) sul thread principale
        unsafe { app.setApplicationIconImage(image.as_deref()) };
    }

    /// Menu del tasto destro sul Dock. winit non lo offre: si aggiunge al delegato
    /// dell'applicazione il metodo `applicationDockMenu:` (macOS lo chiede a ogni clic).
    mod dock {
        use super::super::Status;
        use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
        use objc2::{msg_send, sel};
        use std::cell::RefCell;
        use tray_icon::menu::{ContextMenu, Menu};

        thread_local! {
            static MENU: RefCell<Option<Menu>> = const { RefCell::new(None) };
            static SHOWN: RefCell<Option<Status>> = const { RefCell::new(None) };
        }

        unsafe extern "C-unwind" fn dock_menu(
            _this: *mut AnyObject,
            _cmd: Sel,
            _sender: *mut AnyObject,
        ) -> *mut AnyObject {
            MENU.with(|m| {
                m.borrow()
                    .as_ref()
                    .map(|m| m.ns_menu() as *mut AnyObject)
                    .unwrap_or(std::ptr::null_mut())
            })
        }

        /// Aggiunge il metodo al delegato; false se il delegato non c'è ancora.
        pub fn install() -> bool {
            use objc2_app_kit::NSApplication;
            use objc2_foundation::MainThreadMarker;
            let Some(mtm) = MainThreadMarker::new() else {
                return false;
            };
            let app = NSApplication::sharedApplication(mtm);
            // SAFETY: `delegate` è un normale getter di NSApplication
            let delegate: *mut AnyObject = unsafe { msg_send![&*app, delegate] };
            if delegate.is_null() {
                return false;
            }
            // SAFETY: oggetto Objective-C valido; la firma combacia con "@@:@"
            unsafe {
                let cls: *const AnyClass = (*delegate).class();
                let imp: Imp = std::mem::transmute::<
                    unsafe extern "C-unwind" fn(
                        *mut AnyObject,
                        Sel,
                        *mut AnyObject,
                    ) -> *mut AnyObject,
                    Imp,
                >(dock_menu);
                objc2::ffi::class_addMethod(
                    cls as *mut AnyClass,
                    sel!(applicationDockMenu:),
                    imp,
                    c"@@:@".as_ptr(),
                );
            }
            true
        }

        pub fn wants_update(st: &Status) -> bool {
            SHOWN.with(|s| s.borrow().as_ref() != Some(st))
        }

        pub fn set_menu(menu: Menu) {
            MENU.with(|m| *m.borrow_mut() = Some(menu));
        }

        pub fn shown(st: &Status) {
            SHOWN.with(|s| *s.borrow_mut() = Some(st.clone()));
        }
    }

    /// Numero rosso sull'icona del Dock (None lo toglie). Funzione di macOS dalla 10.5.
    fn dock_badge(label: Option<&str>) {
        use objc2_app_kit::NSApplication;
        use objc2_foundation::{MainThreadMarker, NSString};
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let tile = NSApplication::sharedApplication(mtm).dockTile();
        let text = label.map(NSString::from_str);
        tile.setBadgeLabel(text.as_deref());
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::{Action, Status};

    /// Fuori dal Mac non c'è barra dei menu: non fa niente.
    pub struct MenuBar;

    impl MenuBar {
        pub fn new(_wake: impl Fn() + Send + Sync + 'static) -> Self {
            MenuBar
        }

        pub fn actions(&mut self) -> Vec<Action> {
            Vec::new()
        }

        pub fn update(&mut self, _st: &Status) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha_at(rgba: &[u8], x: usize, y: usize) -> u8 {
        rgba[(y * ICON_PX as usize + x) * 4 + 3]
    }

    #[test]
    fn icons_have_the_right_size_and_shapes() {
        let idle = draw(Look::Idle);
        assert_eq!(idle.len(), (ICON_PX * ICON_PX * 4) as usize);
        // la freccia al centro c'è, gli angoli sono trasparenti
        assert_eq!(alpha_at(&idle, 18, 12), 255);
        assert_eq!(alpha_at(&idle, 0, 0), 0);
        assert_eq!(alpha_at(&idle, 35, 35), 0);
        // la barretta si riempie con l'avanzamento
        let half = draw(Look::progress(0.5));
        let full = draw(Look::progress(1.0));
        assert_eq!(alpha_at(&half, 10, 29), 255);
        assert_eq!(alpha_at(&half, 26, 29), 0);
        assert_eq!(alpha_at(&full, 26, 29), 255);
        // il pallino dell'errore in alto a destra
        assert_eq!(alpha_at(&draw(Look::Failed), 29, 6), 255);
        assert_eq!(alpha_at(&idle, 29, 6), 0);
        // la spunta non ha la freccia
        assert_eq!(alpha_at(&draw(Look::Done), 18, 6), 0);
    }

    #[test]
    fn dock_icon_has_the_progress_bar() {
        let px = |png: &[u8], x: u32, y: u32| {
            image::load_from_memory(png)
                .unwrap()
                .to_rgba8()
                .get_pixel(x, y)
                .0
        };
        let half = dock_icon_png(0.5).unwrap();
        let img = image::load_from_memory(&half).unwrap();
        assert_eq!((img.width(), img.height()), (256, 256));
        // a sinistra bianco pieno, a destra il fondo scuro
        assert_eq!(px(&half, 70, 227), [255, 255, 255, 255]);
        let right = px(&half, 190, 227);
        assert!(right[0] < 200, "fondo scuro atteso, visto {right:?}");
        // sopra la barra l'icona non cambia
        assert_eq!(
            px(&half, 128, 60),
            px(&dock_icon_png(0.0).unwrap(), 128, 60)
        );
    }

    #[test]
    fn progress_is_rounded_to_steps() {
        assert_eq!(Look::progress(0.0), Look::Progress(0));
        assert_eq!(Look::progress(0.54), Look::Progress(6));
        assert_eq!(Look::progress(2.0), Look::Progress(12));
    }

    #[test]
    fn copied_links_go_to_the_right_place() {
        assert_eq!(
            link_kind("magnet:?xt=urn:btih:abc").map(|l| l.0),
            Some(LinkKind::Torrent)
        );
        assert_eq!(
            link_kind(" https://ftp.crifo.org/artix/iso/artix.iso \n").unwrap(),
            (
                LinkKind::File,
                "https://ftp.crifo.org/artix/iso/artix.iso".into()
            )
        );
        assert_eq!(
            link_kind("https://x.org/setup.DMG?dl=1").map(|l| l.0),
            Some(LinkKind::File)
        );
        assert_eq!(
            link_kind("https://youtu.be/abc").map(|l| l.0),
            Some(LinkKind::Media)
        );
        assert_eq!(link_kind("ciao a tutti"), None);
        assert_eq!(link_kind("ftp://x.org/a.iso"), None);
    }

    #[test]
    fn status_texts_and_look() {
        let mut st = Status::default();
        assert_eq!(st.look(), Look::Idle);
        assert_eq!(st.title(), None);
        st.look_failed = true;
        assert_eq!(st.look(), Look::Failed);
        st.active = 2;
        st.fraction = Some(0.54);
        st.speed = 3.0 * 1024.0 * 1024.0;
        assert_eq!(st.look(), Look::Progress(6));
        assert_eq!(st.title().as_deref(), Some("54%"));
        assert!(st.header().contains("2 "));
        assert!(st.header().contains("MB/s"));
    }

    #[test]
    fn actions_round_trip_through_menu_ids() {
        for a in [
            Action::DownloadClipboard,
            Action::PauseAll,
            Action::ResumeAll,
            Action::SpeedLimit(0),
            Action::SpeedLimit(5120),
            Action::OpenApp,
            Action::OpenFolder,
            Action::Quit,
        ] {
            assert_eq!(Action::from_id(&a.id()), Some(a));
        }
        assert_eq!(Action::from_id("row"), Some(Action::OpenApp));
        assert_eq!(Action::from_id("header"), None);
        assert_eq!(limit_label(1024), "1 MB/s");
        assert_eq!(limit_label(256), "256 KB/s");
    }
}
