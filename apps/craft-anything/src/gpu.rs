//! Linux: which graphics adapter draws the window (#571).
//!
//! egui-wgpu asks for the high-performance adapter, so on a laptop with two GPUs the window was
//! drawn on the discrete one while the integrated one drives the screen: 20 s to open and a lagging
//! desktop (every frame is copied across). Here the adapter is the GPU that drives the screen:
//!
//! 1. `WGPU_ADAPTER_NAME` (an adapter whose name contains it), then `WGPU_POWER_PREF`, decide when set.
//! 2. A built-in panel (`eDP`, `LVDS`, `DSI`) that is connected: the GPU that drives it.
//! 3. Displays on one GPU only: that GPU.
//! 4. Displays on several GPUs, or no way to tell: the discrete GPU, as before.
//!
//! The choice is a pure function of the adapters, the environment and the connected displays, so
//! each rule has a test. Displays are read from `/sys/class/drm` once, as the app starts.

use eframe::wgpu::{Adapter, Backend, DeviceType, PowerPreference, Surface};
use std::path::Path;

/// What the choice needs to know about one adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gpu {
    pub name: String,
    pub backend: Backend,
    pub kind: DeviceType,
    /// PCI vendor and device ids (zero for a backend that doesn't report them).
    pub vendor: u32,
    pub device: u32,
}

/// A connected display and the PCI id of the GPU that drives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Display {
    pub connector: String,
    pub vendor: u32,
    pub device: u32,
}

/// What the environment says.
#[derive(Debug, Clone, Default)]
pub struct Env {
    /// `WGPU_ADAPTER_NAME`.
    pub adapter_name: Option<String>,
    /// `WGPU_POWER_PREF`.
    pub power: Option<PowerPreference>,
}

impl Env {
    pub fn from_process() -> Env {
        Env { adapter_name: std::env::var("WGPU_ADAPTER_NAME").ok().filter(|v| !v.trim().is_empty()), power: PowerPreference::from_env() }
    }
}

/// The adapter for the window: its index in `gpus` and why.
pub fn choose(gpus: &[Gpu], env: &Env, displays: &[Display]) -> Result<(usize, &'static str), String> {
    if gpus.is_empty() {
        return Err("No graphics adapter can draw this window.".into());
    }
    if let Some(wanted) = &env.adapter_name {
        let wanted = wanted.to_lowercase();
        return match best(gpus, |g| g.name.to_lowercase().contains(&wanted)) {
            Some(i) => Ok((i, "WGPU_ADAPTER_NAME")),
            None => Err(format!("WGPU_ADAPTER_NAME is {wanted:?}, but no graphics adapter has that in its name.")),
        };
    }
    if let Some(power) = env.power {
        let pick = match power {
            PowerPreference::None => best(gpus, |_| true).map(|i| (i, "WGPU_POWER_PREF=none")),
            PowerPreference::LowPower => best(gpus, |g| g.kind == DeviceType::IntegratedGpu).map(|i| (i, "WGPU_POWER_PREF=low")),
            PowerPreference::HighPerformance => best(gpus, |g| g.kind == DeviceType::DiscreteGpu).map(|i| (i, "WGPU_POWER_PREF=high")),
        };
        if let Some(found) = pick {
            return Ok(found);
        }
    }
    if let Some((vendor, device)) = screen_gpu(displays)
        && let Some(i) = best(gpus, |g| g.vendor == vendor && g.device == device)
    {
        return Ok((i, "the GPU that drives the screen"));
    }
    best(gpus, |_| true).map(|i| (i, "the fastest GPU")).ok_or_else(|| "No graphics adapter can draw this window.".into())
}

/// The GPU that drives the screen, when the displays say: the one behind a connected built-in
/// panel, else the only one that has displays.
fn screen_gpu(displays: &[Display]) -> Option<(u32, u32)> {
    let builtin = displays.iter().find(|d| ["eDP", "LVDS", "DSI"].iter().any(|k| d.connector.contains(k)));
    if let Some(d) = builtin {
        return Some((d.vendor, d.device));
    }
    let first = displays.first()?;
    displays.iter().all(|d| (d.vendor, d.device) == (first.vendor, first.device)).then_some((first.vendor, first.device))
}

/// The best adapter that `ok` accepts: a real GPU before a software one, Vulkan before the rest
/// (GL lists the same card again), discrete before integrated, then the order they came in.
fn best(gpus: &[Gpu], ok: impl Fn(&Gpu) -> bool) -> Option<usize> {
    let rank = |g: &Gpu| {
        let kind = match g.kind {
            DeviceType::DiscreteGpu => 0,
            DeviceType::IntegratedGpu => 1,
            DeviceType::VirtualGpu => 2,
            DeviceType::Cpu => 3,
            DeviceType::Other => 4,
        };
        (g.kind == DeviceType::Cpu, g.backend != Backend::Vulkan, kind)
    };
    gpus.iter().enumerate().filter(|(_, g)| ok(g)).min_by_key(|(i, g)| (rank(g), *i)).map(|(i, _)| i)
}

/// The connected displays under `drm` (`/sys/class/drm`), each with the PCI id of the GPU behind
/// it. What can't be read is left out: no displays is no opinion.
pub fn displays_in(drm: &Path) -> Vec<Display> {
    let Ok(entries) = std::fs::read_dir(drm) else { return Vec::new() };
    let mut names: Vec<String> = entries.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect();
    names.sort();
    let hex = |p: std::path::PathBuf| {
        let text = std::fs::read_to_string(p).ok()?;
        u32::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok()
    };
    let mut out = Vec::new();
    for name in names {
        // `card1-eDP-1`: the card, then the connector.
        let Some((card, connector)) = name.split_once('-') else { continue };
        if !card.starts_with("card") {
            continue;
        }
        let connected = std::fs::read_to_string(drm.join(&name).join("status")).is_ok_and(|s| s.trim() == "connected");
        if !connected {
            continue;
        }
        let device_dir = drm.join(card).join("device");
        if let (Some(vendor), Some(device)) = (hex(device_dir.join("vendor")), hex(device_dir.join("device"))) {
            out.push(Display { connector: connector.to_string(), vendor, device });
        }
    }
    out
}

/// The adapter selector for eframe: [`choose`] over the adapters that can draw to the window.
pub fn pick_adapter(adapters: &[Adapter], surface: Option<&Surface<'_>>) -> Result<Adapter, String> {
    let usable: Vec<&Adapter> = adapters.iter().filter(|a| surface.is_none_or(|s| a.is_surface_supported(s))).collect();
    let gpus: Vec<Gpu> = usable
        .iter()
        .map(|a| {
            let info = a.get_info();
            Gpu { name: info.name, backend: info.backend, kind: info.device_type, vendor: info.vendor, device: info.device }
        })
        .collect();
    let displays = displays_in(Path::new("/sys/class/drm"));
    log::info!("displays: {displays:?}");
    let (i, why) = choose(&gpus, &Env::from_process(), &displays)?;
    let picked = usable.get(i).ok_or_else(|| "No graphics adapter can draw this window.".to_string())?;
    log::info!("drawing with {:?} ({why})", picked.get_info());
    Ok((*picked).clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    const AMD: (u32, u32) = (0x1002, 0x1636);
    const NVIDIA: (u32, u32) = (0x10de, 0x25a2);

    fn gpu(name: &str, backend: Backend, kind: DeviceType, id: (u32, u32)) -> Gpu {
        Gpu { name: name.into(), backend, kind, vendor: id.0, device: id.1 }
    }

    /// The laptop in #571: Vulkan and GL list each card, the discrete one first.
    fn laptop() -> Vec<Gpu> {
        vec![
            gpu("NVIDIA GeForce RTX 3050", Backend::Vulkan, DeviceType::DiscreteGpu, NVIDIA),
            gpu("AMD Radeon Graphics (RADV RENOIR)", Backend::Vulkan, DeviceType::IntegratedGpu, AMD),
            gpu("NVIDIA GeForce RTX 3050", Backend::Gl, DeviceType::DiscreteGpu, (0, 0)),
            gpu("llvmpipe", Backend::Vulkan, DeviceType::Cpu, (0x10005, 0)),
        ]
    }

    fn on(connector: &str, id: (u32, u32)) -> Display {
        Display { connector: connector.into(), vendor: id.0, device: id.1 }
    }

    fn name(gpus: &[Gpu], c: Result<(usize, &'static str), String>) -> String {
        gpus[c.unwrap().0].name.clone()
    }

    /// #571: the built-in panel is on the integrated GPU, so it draws the window.
    #[test]
    fn the_gpu_behind_the_built_in_panel_draws_the_window() {
        let g = laptop();
        let d = [on("eDP-1", AMD)];
        assert_eq!(name(&g, choose(&g, &Env::default(), &d)), "AMD Radeon Graphics (RADV RENOIR)");
        // With an external monitor on the other GPU, the panel still decides.
        let d = [on("eDP-1", AMD), on("HDMI-A-1", NVIDIA)];
        assert_eq!(name(&g, choose(&g, &Env::default(), &d)), "AMD Radeon Graphics (RADV RENOIR)");
        // A laptop whose panel is on the discrete GPU (a MUX switch) keeps it.
        let d = [on("eDP-1", NVIDIA)];
        let (i, _) = choose(&g, &Env::default(), &d).unwrap();
        assert_eq!((g[i].name.as_str(), g[i].backend), ("NVIDIA GeForce RTX 3050", Backend::Vulkan), "Vulkan, not the GL copy");
    }

    /// A desktop: displays on one GPU decide, and on several (or none) it's the discrete GPU as before.
    #[test]
    fn displays_on_one_gpu_decide_and_otherwise_it_is_the_discrete_gpu() {
        let g = laptop();
        assert_eq!(name(&g, choose(&g, &Env::default(), &[on("DP-1", AMD), on("DP-2", AMD)])), "AMD Radeon Graphics (RADV RENOIR)");
        assert_eq!(name(&g, choose(&g, &Env::default(), &[on("DP-1", NVIDIA)])), "NVIDIA GeForce RTX 3050");
        assert_eq!(name(&g, choose(&g, &Env::default(), &[on("DP-1", AMD), on("DP-2", NVIDIA)])), "NVIDIA GeForce RTX 3050", "one monitor on each");
        assert_eq!(name(&g, choose(&g, &Env::default(), &[])), "NVIDIA GeForce RTX 3050", "no way to tell");
        // A display on a GPU that isn't among the adapters leaves it to the discrete GPU too.
        assert_eq!(name(&g, choose(&g, &Env::default(), &[on("DP-1", (0x8086, 0x9a49))])), "NVIDIA GeForce RTX 3050");
        // And software rendering is never preferred over a real GPU.
        let only_soft = [gpu("llvmpipe", Backend::Vulkan, DeviceType::Cpu, (0x10005, 0))];
        assert_eq!(choose(&only_soft, &Env::default(), &[]).unwrap().0, 0);
        assert!(choose(&[], &Env::default(), &[]).is_err());
    }

    /// The environment comes first: `WGPU_ADAPTER_NAME` (which egui-wgpu ignores) and `WGPU_POWER_PREF`.
    #[test]
    fn the_environment_decides_when_it_is_set() {
        let g = laptop();
        let panel = [on("eDP-1", AMD)];
        let by_name = Env { adapter_name: Some("nvidia".into()), ..Env::default() };
        assert_eq!(name(&g, choose(&g, &by_name, &panel)), "NVIDIA GeForce RTX 3050");
        let missing = Env { adapter_name: Some("intel".into()), ..Env::default() };
        assert!(choose(&g, &missing, &panel).unwrap_err().contains("WGPU_ADAPTER_NAME"));
        let high = Env { power: Some(PowerPreference::HighPerformance), ..Env::default() };
        assert_eq!(name(&g, choose(&g, &high, &panel)), "NVIDIA GeForce RTX 3050");
        let low = Env { power: Some(PowerPreference::LowPower), ..Env::default() };
        assert_eq!(name(&g, choose(&g, &low, &[])), "AMD Radeon Graphics (RADV RENOIR)");
        let none = Env { power: Some(PowerPreference::None), ..Env::default() };
        assert_eq!(choose(&g, &none, &panel).unwrap().0, 0, "the first one listed");
        // A preference nothing satisfies falls through to the screen's GPU.
        let only_amd = [g[1].clone()];
        assert_eq!(choose(&only_amd, &high, &panel).unwrap().0, 0);
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// `/sys/class/drm` as the kernel lays it out: `cardN/device/{vendor,device}` and
    /// `cardN-<connector>/status`.
    #[test]
    fn the_connected_displays_are_read_from_a_drm_tree() {
        let d = tempfile::tempdir().unwrap();
        let drm = d.path();
        write(&drm.join("card0/device/vendor"), "0x10de\n");
        write(&drm.join("card0/device/device"), "0x25a2\n");
        write(&drm.join("card1/device/vendor"), "0x1002\n");
        write(&drm.join("card1/device/device"), "0x1636\n");
        write(&drm.join("card0-HDMI-A-1/status"), "disconnected\n");
        write(&drm.join("card1-eDP-1/status"), "connected\n");
        write(&drm.join("card1-DP-3/status"), "connected\n");
        write(&drm.join("card0-DP-1/status"), "unknown\n");
        // Not a card or not a connector, and a card whose ids can't be read.
        write(&drm.join("renderD128/status"), "connected\n");
        write(&drm.join("version"), "x\n");
        write(&drm.join("card2-DP-9/status"), "connected\n");
        let got = displays_in(drm);
        assert_eq!(got, vec![on("DP-3", AMD), on("eDP-1", AMD)]);
        assert!(displays_in(&drm.join("missing")).is_empty());
        let g = laptop();
        assert_eq!(name(&g, choose(&g, &Env::default(), &got)), "AMD Radeon Graphics (RADV RENOIR)");
    }
}
