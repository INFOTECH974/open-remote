//! Capture de l'écran et encodage en tuiles JPEG.
//!
//! L'écran est découpé en tuiles de 128×128 px ; seules les tuiles modifiées
//! depuis l'image précédente sont encodées et envoyées. Le visualiseur
//! accuse réception de chaque image : on n'en envoie jamais plus de
//! [`MAX_IN_FLIGHT`] d'avance, ce qui adapte la cadence au débit réel et évite
//! l'accumulation de latence.

use std::{
    borrow::Cow,
    sync::{
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
        mpsc::{Receiver, RecvTimeoutError},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use anyhow::{bail, Context};
use distant_proto::session::{
    encode_frame, MonitorInfo, Tile, QUALITY_BALANCED, QUALITY_FAST, QUALITY_HIGH,
};
use jpeg_encoder::{ColorType, Encoder, SamplingFactor};
use rayon::prelude::*;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use xcap::Monitor;

const TILE: u32 = 128;
const MAX_IN_FLIGHT: u32 = 2;
/// Cadence maximale : environ 30 images par seconde.
const MIN_FRAME_INTERVAL: Duration = Duration::from_millis(33);
/// Intervalle de vérification quand l'écran ne change pas.
const IDLE_INTERVAL: Duration = Duration::from_millis(50);

/// Position et taille de l'écran partagé, dans le repère utilisé pour
/// l'injection de la souris (pixels physiques sous Windows et Linux,
/// points sous macOS : c'est ce que renvoie `xcap`).
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Réglages partagés entre la session (réseau), la capture et l'injection.
pub struct CaptureSettings {
    monitor: AtomicUsize,
    quality: AtomicU8,
    dirty: AtomicBool,
    geometry: Mutex<Geometry>,
}

impl CaptureSettings {
    pub fn new() -> Self {
        let monitors = Monitor::all().unwrap_or_default();
        let primary = monitors.iter().position(|m| m.is_primary().unwrap_or(false)).unwrap_or(0);
        let geometry = monitors
            .get(primary)
            .map(geometry_of)
            .unwrap_or(Geometry { x: 0, y: 0, width: 1, height: 1 });
        Self {
            monitor: AtomicUsize::new(primary),
            quality: AtomicU8::new(QUALITY_BALANCED),
            dirty: AtomicBool::new(true),
            geometry: Mutex::new(geometry),
        }
    }

    pub fn set_monitor(&self, index: usize) {
        self.monitor.store(index, Ordering::Relaxed);
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn set_quality(&self, quality: u8) {
        if self.quality.swap(quality, Ordering::Relaxed) != quality {
            self.dirty.store(true, Ordering::Relaxed);
        }
    }

    pub fn geometry(&self) -> Geometry {
        *self.geometry.lock().unwrap()
    }
}

fn geometry_of(m: &Monitor) -> Geometry {
    Geometry {
        x: m.x().unwrap_or(0),
        y: m.y().unwrap_or(0),
        width: m.width().unwrap_or(1).max(1),
        height: m.height().unwrap_or(1).max(1),
    }
}

pub fn list_monitors() -> Vec<MonitorInfo> {
    Monitor::all()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .map(|(i, m)| MonitorInfo {
            name: m
                .friendly_name()
                .ok()
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| format!("Écran {}", i + 1)),
            width: m.width().unwrap_or(0),
            height: m.height().unwrap_or(0),
            primary: m.is_primary().unwrap_or(i == 0),
        })
        .collect()
}

/// Démarre le thread de capture. Il s'arrête avec la session (`cancel`).
pub fn spawn(
    settings: Arc<CaptureSettings>,
    out: mpsc::Sender<Vec<u8>>,
    acks: Receiver<u32>,
    cancel: CancellationToken,
) {
    let result = std::thread::Builder::new().name("distant-capture".into()).spawn(move || {
        if let Err(e) = capture_loop(&settings, &out, &acks, &cancel) {
            tracing::error!("capture arrêtée : {e:#}");
            cancel.cancel();
        }
    });
    if let Err(e) = result {
        tracing::error!("impossible de lancer la capture : {e}");
    }
}

/// Image RGBA.
struct Frame {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

struct Preset {
    jpeg_quality: u8,
    sampling: SamplingFactor,
    /// Au-delà de cette largeur, l'image est réduite de moitié.
    downscale_above: u32,
}

fn preset(quality: u8) -> Preset {
    match quality {
        QUALITY_FAST => Preset { jpeg_quality: 50, sampling: SamplingFactor::R_4_2_0, downscale_above: 1600 },
        QUALITY_HIGH => Preset { jpeg_quality: 85, sampling: SamplingFactor::R_4_4_4, downscale_above: u32::MAX },
        _ => Preset { jpeg_quality: 70, sampling: SamplingFactor::R_4_2_0, downscale_above: 2560 },
    }
}

fn capture_loop(
    settings: &CaptureSettings,
    out: &mpsc::Sender<Vec<u8>>,
    acks: &Receiver<u32>,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    let mut seq: u32 = 0;
    let mut acked: u32 = 0;
    let mut monitors: Vec<Monitor> = Vec::new();
    let mut current: Option<usize> = None;
    let mut prev: Option<Frame> = None;
    let mut failures: u32 = 0;

    while !cancel.is_cancelled() {
        while let Ok(a) = acks.try_recv() {
            acked = acked.max(a);
        }
        if seq.wrapping_sub(acked) >= MAX_IN_FLIGHT {
            match acks.recv_timeout(Duration::from_millis(250)) {
                Ok(a) => acked = acked.max(a),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            continue;
        }

        let started = Instant::now();
        if settings.dirty.swap(false, Ordering::Relaxed) || current.is_none() {
            monitors = Monitor::all().context("liste des écrans")?;
            if monitors.is_empty() {
                bail!("aucun écran détecté");
            }
            let index = settings.monitor.load(Ordering::Relaxed).min(monitors.len() - 1);
            *settings.geometry.lock().unwrap() = geometry_of(&monitors[index]);
            current = Some(index);
            prev = None; // image complète au prochain envoi
        }
        let Some(monitor) = current.and_then(|i| monitors.get(i)) else { continue };

        let image = match monitor.capture_image() {
            Ok(image) => {
                failures = 0;
                image
            }
            Err(e) => {
                // Écran verrouillé, invite UAC, changement de résolution… on réessaie.
                failures += 1;
                if failures == 1 || failures.is_multiple_of(40) {
                    tracing::warn!("capture impossible : {e}");
                }
                if failures.is_multiple_of(8) {
                    current = None;
                }
                std::thread::sleep(Duration::from_millis(250));
                continue;
            }
        };

        let preset = preset(settings.quality.load(Ordering::Relaxed));
        let frame = prepare(image, &preset);
        let rects = changed_rects(prev.as_ref(), &frame);
        if rects.is_empty() {
            sleep_remaining(started, IDLE_INTERVAL);
            continue;
        }

        let tiles = rects
            .par_iter()
            .map(|r| encode_tile(&frame, r, &preset))
            .collect::<anyhow::Result<Vec<_>>>()?;
        seq = seq.wrapping_add(1);
        let payload = encode_frame(seq, frame.width as u16, frame.height as u16, &tiles);
        if out.blocking_send(payload).is_err() {
            break; // session terminée
        }
        prev = Some(frame);
        sleep_remaining(started, MIN_FRAME_INTERVAL);
    }
    Ok(())
}

fn sleep_remaining(started: Instant, interval: Duration) {
    if let Some(rest) = interval.checked_sub(started.elapsed()) {
        std::thread::sleep(rest);
    }
}

fn prepare(image: xcap::image::RgbaImage, preset: &Preset) -> Frame {
    let (width, height) = image.dimensions();
    // Les dimensions voyagent sur 16 bits : on réduit les écrans géants.
    let too_big = width > u16::MAX as u32 || height > u16::MAX as u32;
    let data = image.into_raw();
    if (width > preset.downscale_above || too_big) && width >= 2 && height >= 2 {
        downscale_half(&data, width, height)
    } else {
        Frame { width, height, data }
    }
}

/// Réduction de moitié (moyenne de 2×2 pixels).
fn downscale_half(src: &[u8], width: u32, height: u32) -> Frame {
    let (nw, nh) = (width / 2, height / 2);
    let src_stride = width as usize * 4;
    let dst_stride = nw as usize * 4;
    let mut data = vec![0u8; dst_stride * nh as usize];
    data.par_chunks_mut(dst_stride).enumerate().for_each(|(y, dst)| {
        let r0 = &src[2 * y * src_stride..][..src_stride];
        let r1 = &src[(2 * y + 1) * src_stride..][..src_stride];
        for x in 0..nw as usize {
            let i = x * 8;
            for c in 0..4 {
                let sum = r0[i + c] as u16 + r0[i + 4 + c] as u16 + r1[i + c] as u16 + r1[i + 4 + c] as u16;
                dst[x * 4 + c] = ((sum + 2) / 4) as u8;
            }
        }
    });
    Frame { width: nw, height: nh, data }
}

/// Zones à envoyer : tuiles modifiées, ou bandes couvrant tout l'écran si
/// plus de la moitié a changé (moins d'en-têtes JPEG, encodage parallèle).
fn changed_rects(prev: Option<&Frame>, cur: &Frame) -> Vec<Rect> {
    let same_size = prev.filter(|p| p.width == cur.width && p.height == cur.height);
    let Some(prev) = same_size else {
        return full_bands(cur);
    };

    let cols = cur.width.div_ceil(TILE);
    let rows = cur.height.div_ceil(TILE);
    let changed: Vec<Rect> = (0..rows * cols)
        .into_par_iter()
        .filter_map(|i| {
            let x = (i % cols) * TILE;
            let y = (i / cols) * TILE;
            let r = Rect { x, y, w: TILE.min(cur.width - x), h: TILE.min(cur.height - y) };
            differs(prev, cur, &r).then_some(r)
        })
        .collect();

    let changed_area: u64 = changed.iter().map(|r| r.w as u64 * r.h as u64).sum();
    if changed_area * 2 > cur.width as u64 * cur.height as u64 {
        full_bands(cur)
    } else {
        changed
    }
}

/// Découpe l'écran en bandes horizontales (une par cœur, hauteur multiple de 16).
fn full_bands(frame: &Frame) -> Vec<Rect> {
    let bands = rayon::current_num_threads().clamp(1, 16) as u32;
    let band_h = frame.height.div_ceil(bands).div_ceil(16).max(1) * 16;
    (0..frame.height)
        .step_by(band_h as usize)
        .map(|y| Rect { x: 0, y, w: frame.width, h: band_h.min(frame.height - y) })
        .collect()
}

fn differs(a: &Frame, b: &Frame, r: &Rect) -> bool {
    let stride = a.width as usize * 4;
    let len = r.w as usize * 4;
    (r.y..r.y + r.h).any(|row| {
        let off = row as usize * stride + r.x as usize * 4;
        a.data[off..off + len] != b.data[off..off + len]
    })
}

fn encode_tile(frame: &Frame, r: &Rect, preset: &Preset) -> anyhow::Result<Tile> {
    let stride = frame.width as usize * 4;
    let row_len = r.w as usize * 4;
    let pixels: Cow<[u8]> = if r.x == 0 && r.w == frame.width {
        let start = r.y as usize * stride;
        Cow::Borrowed(&frame.data[start..start + r.h as usize * stride])
    } else {
        let mut buf = Vec::with_capacity(row_len * r.h as usize);
        for row in r.y..r.y + r.h {
            let off = row as usize * stride + r.x as usize * 4;
            buf.extend_from_slice(&frame.data[off..off + row_len]);
        }
        Cow::Owned(buf)
    };

    let mut jpeg = Vec::with_capacity(row_len * r.h as usize / 8);
    let mut encoder = Encoder::new(&mut jpeg, preset.jpeg_quality);
    encoder.set_sampling_factor(preset.sampling);
    encoder.encode(&pixels, r.w as u16, r.h as u16, ColorType::Rgba)?;
    Ok(Tile { x: r.x as u16, y: r.y as u16, w: r.w as u16, h: r.h as u16, jpeg })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32, fill: u8) -> Frame {
        Frame { width, height, data: vec![fill; (width * height * 4) as usize] }
    }

    #[test]
    fn first_frame_is_full() {
        let f = frame(300, 200, 0);
        let rects = changed_rects(None, &f);
        let area: u32 = rects.iter().map(|r| r.w * r.h).sum();
        assert_eq!(area, 300 * 200);
    }

    #[test]
    fn identical_frames_send_nothing() {
        let a = frame(300, 200, 7);
        let b = frame(300, 200, 7);
        assert!(changed_rects(Some(&a), &b).is_empty());
    }

    #[test]
    fn small_change_sends_one_tile() {
        let a = frame(1000, 600, 0);
        let mut b = frame(1000, 600, 0);
        // Pixel (130, 5) -> tuile (128, 0).
        let off = ((5 * 1000 + 130) * 4) as usize;
        b.data[off] = 255;
        let rects = changed_rects(Some(&a), &b);
        assert_eq!(rects, vec![Rect { x: 128, y: 0, w: 128, h: 128 }]);
    }

    #[test]
    fn edge_tiles_are_clipped() {
        let a = frame(130, 130, 0);
        let mut b = frame(130, 130, 0);
        let last = b.data.len() - 1;
        b.data[last] = 1;
        let rects = changed_rects(Some(&a), &b);
        assert_eq!(rects, vec![Rect { x: 128, y: 128, w: 2, h: 2 }]);
    }

    #[test]
    fn downscale_averages() {
        let mut f = frame(4, 2, 0);
        f.data[0] = 200; // pixel (0,0), rouge
        f.data[4] = 100; // pixel (1,0), rouge
        let small = downscale_half(&f.data, 4, 2);
        assert_eq!((small.width, small.height), (2, 1));
        assert_eq!(small.data[0], 75); // (200 + 100 + 0 + 0) / 4
    }

    #[test]
    fn tiles_encode_to_jpeg() {
        let f = frame(200, 100, 128);
        let tile = encode_tile(&f, &Rect { x: 128, y: 0, w: 72, h: 100 }, &preset(QUALITY_BALANCED)).unwrap();
        assert_eq!(&tile.jpeg[..2], &[0xFF, 0xD8]); // marqueur SOI
    }
}
