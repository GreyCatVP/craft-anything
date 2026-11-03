//! Sound for Xbox titles the backend's DirectSound replacement doesn't fully cover, XACT ones
//! above all. Xbox XBE Recomp runs DirectSound on the PC (its objects are the host's), so any of
//! the title's own DirectSound code that still runs touches hardware or objects that aren't there.
//! Three things fill the gaps, written into the build's own sources (the backend stays as
//! upstream has it):
//!
//! - the audio processor's sample counter (XGSCNT), which XACT, XPP and DirectSound read for
//!   timing, runs at 48 kHz from a host thread;
//! - the HRTF pointer table `DirectSoundUse*HRTF` would have filled is filled at load, so the 3D
//!   maths XACT runs itself has its routines;
//! - DirectSound functions the symbol scan couldn't place are found next to ones it did, checked
//!   by their argument count, then named for the backend's versions or given host stand-ins.

use craft_anything_formats::xbe::{self, Xbe};
use iced_x86::{Decoder, DecoderOptions, Mnemonic};
use std::collections::HashMap;

/// What a recognised function gets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Named in `[symbols]`, so the backend's version runs.
    Map(&'static str),
    /// Returns S_OK: a setter with nothing to set on the PC (pitch, LFO, envelopes, 3D extras).
    Accept,
    /// Clears the first argument's struct (this many bytes) and returns S_OK: a getter.
    Clear(u32),
    /// `IDirectSound_CreateSoundStream(this, desc, stream, unknown)`: the backend's `DirectSoundCreateStream`.
    CreateStream,
    /// `SetPitch(this, pitch)`: the pitch (1/4096 octaves from 48 kHz) as a rate for the backend's
    /// `SetFrequency` of this name; XACT sets every sound's playback rate this way.
    Pitch(&'static str),
}

struct Known {
    anchor: &'static str,
    delta: i32,
    pops: u32,
    thunk: bool,
    what: &'static str,
    action: Action,
}

const fn k(anchor: &'static str, delta: i32, pops: u32, thunk: bool, what: &'static str, action: Action) -> Known {
    Known { anchor, delta, pops, thunk, what, action }
}

// shortcut: offsets measured in one title's DSOUND 1.0.5849 (The Warriors, an XACT title); each is
// checked by argument count and kind before use, so a different layout is skipped, not mis-hooked.
// Grow it (or derive it from the code) as more titles are run.
static DSOUND_5849: &[Known] = &[
    k("IDirectSoundBuffer_SetVolume", 0x1C, 2, false, "IDirectSoundBuffer_SetPitch", Action::Pitch("IDirectSoundBuffer_SetFrequency")),
    k("IDirectSoundBuffer_SetVolume", 0x38, 2, false, "IDirectSoundBuffer_SetLFO", Action::Accept),
    k("IDirectSoundBuffer_SetVolume", 0x54, 2, false, "IDirectSoundBuffer_SetEG", Action::Map("IDirectSoundBuffer_SetEG")),
    k("IDirectSoundBuffer_SetVolume", 0x70, 2, false, "IDirectSoundBuffer_SetFilter", Action::Map("IDirectSoundBuffer_SetFilter")),
    k("IDirectSoundBuffer_Stop", 0x18, 4, false, "IDirectSoundBuffer_StopEx", Action::Map("IDirectSoundBuffer_StopEx")),
    k("IDirectSoundBuffer_SetFrequency", 0x1C, 2, false, "IDirectSoundBuffer_SetOutputBuffer", Action::Map("IDirectSoundBuffer_SetOutputBuffer")),
    k("IDirectSoundBuffer_SetBufferData", -0x1C, 2, false, "IDirectSoundBuffer_SetFormat", Action::Map("IDirectSoundBuffer_SetFormat")),
    k("IDirectSoundBuffer_Lock", 0x50, 2, false, "IDirectSoundBuffer_GetVoiceProperties", Action::Clear(0x48)),
    k("IDirectSoundBuffer_Lock", 0x6C, 2, false, "an IDirectSoundBuffer setter", Action::Accept),
    k("IDirectSoundBuffer_Lock", 0x88, 2, false, "an IDirectSoundBuffer setter", Action::Accept),
    k("IDirectSoundBuffer_SetI3DL2Source", 0x20, 3, false, "IDirectSoundBuffer_SetPlayRegion", Action::Map("IDirectSoundBuffer_SetPlayRegion")),
    k("IDirectSoundBuffer_SetI3DL2Source", 0x45, 2, true, "IDirectSoundStream_SetOutputBuffer", Action::Accept),
    k("IDirectSound_GetSpeakerConfig", 0x1C, 5, false, "IDirectSound_DownloadEffectsImage", Action::Map("IDirectSound_DownloadEffectsImage")),
    k("IDirectSound_SetMixBinHeadroom", 0x3C, 3, false, "IDirectSound_GetOutputLevels", Action::Map("IDirectSound_GetOutputLevels")),
    k("IDirectSound_CreateSoundBuffer", 0x24, 4, false, "IDirectSound_CreateSoundStream", Action::CreateStream),
    k("DirectSoundDoWork", -0x91, 2, true, "IDirectSoundStream_SetPitch", Action::Pitch("CDirectSoundStream_SetFrequency")),
    k("DirectSoundDoWork", -0x8C, 2, true, "IDirectSoundStream_SetLFO", Action::Accept),
    k("DirectSoundDoWork", -0x87, 2, true, "IDirectSoundStream_SetEG", Action::Map("IDirectSoundStream_SetEG")),
    k("DirectSoundDoWork", -0x3E, 2, true, "IDirectSoundStream_GetVoiceProperties", Action::Clear(0x48)),
    k("DirectSoundDoWork", -0x39, 2, true, "an IDirectSoundStream setter", Action::Accept),
    k("DirectSoundDoWork", -0x34, 2, true, "an IDirectSoundStream setter", Action::Accept),
];

/// A host stand-in for a guest function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hook {
    pub at: u32,
    pub what: &'static str,
    pub pops: u32,
    pub action: Action,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// `[symbols]` entries to set.
    pub symbols: Vec<(&'static str, u32)>,
    pub hooks: Vec<Hook>,
    /// Guest words to write at load (the HRTF table).
    pub pokes: Vec<(u32, u32)>,
    /// Table rows that didn't check out in this title.
    pub skipped: usize,
}

/// The `NAME = 0xADDR` entries of a config's `[symbols]` section (0 means not found).
pub fn symbols(config: &str) -> HashMap<String, u32> {
    let mut out = HashMap::new();
    let mut inside = false;
    for line in config.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == "[symbols]";
        } else if inside && let Some((k, v)) = line.split_once('=') {
            let v = v.split('#').next().unwrap_or("").trim();
            if let Ok(at) = u32::from_str_radix(v.trim_start_matches("0x").trim_start_matches("0X"), 16) {
                out.insert(k.trim().to_string(), at);
            }
        }
    }
    out
}

fn section<'a>(x: &'a Xbe, name: &str) -> Option<&'a xbe::Section> {
    x.sections.iter().find(|s| s.name == name)
}

/// The file's bytes at a virtual address, at most `n`.
fn at<'a>(file: &'a [u8], x: &Xbe, va: u32, n: usize) -> Option<&'a [u8]> {
    let s = x.sections.iter().find(|s| va.checked_sub(s.virtual_address).is_some_and(|o| o < s.raw_size))?;
    let off = va - s.virtual_address;
    let start = (s.raw_offset as usize).checked_add(off as usize)?;
    let len = n.min((s.raw_size - off) as usize);
    file.get(start..start.checked_add(len)?)
}

/// Where a `jmp rel32` at `va` goes.
fn thunk_target(file: &[u8], x: &Xbe, va: u32) -> Option<u32> {
    let b = at(file, x, va, 5)?;
    let rel: [u8; 4] = b.get(1..5)?.try_into().ok()?;
    (b.first() == Some(&0xE9)).then(|| va.wrapping_add(5).wrapping_add(u32::from_le_bytes(rel)))
}

/// Stack words the function at `va` pops (from its first `ret`), following a leading thunk.
fn pops(file: &[u8], x: &Xbe, va: u32) -> Option<u32> {
    let body = thunk_target(file, x, va).unwrap_or(va);
    let code = at(file, x, body, 0x800)?;
    let mut d = Decoder::with_ip(32, code, u64::from(body), DecoderOptions::NONE);
    for _ in 0..512 {
        if !d.can_decode() {
            return None;
        }
        let i = d.decode();
        if i.is_invalid() {
            return None;
        }
        if i.mnemonic() == Mnemonic::Ret {
            return Some(u32::from(i.immediate16()) / 4);
        }
    }
    None
}

/// The HRTF table: a run of `mov dword [abs], imm32` ending in `ret`, in DSOUND, writing a pointer
/// DSOUND also calls or jumps through. The first such run (full or light HRTF) is used.
fn hrtf(file: &[u8], x: &Xbe, s: &xbe::Section) -> Vec<(u32, u32)> {
    let Some(code) = at(file, x, s.virtual_address, s.raw_size as usize) else { return Vec::new() };
    let word = |i: usize| code.get(i..i + 4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
    let indirect = |p: u32| {
        let needle = p.to_le_bytes();
        code.windows(6).any(|w| matches!(w, [0xFF, 0x15 | 0x25, rest @ ..] if *rest == needle))
    };
    let mut i = 0;
    while i + 10 < code.len() {
        let mut writes = Vec::new();
        let mut j = i;
        while code.get(j..j + 2) == Some(&[0xC7, 0x05]) {
            match (word(j + 2), word(j + 6)) {
                (Some(a), Some(v)) => writes.push((a, v)),
                _ => break,
            }
            j += 10;
        }
        if writes.len() >= 8 && code.get(j) == Some(&0xC3) && writes.iter().any(|&(a, _)| indirect(a)) {
            return writes;
        }
        i = if writes.is_empty() { i + 1 } else { j };
    }
    Vec::new()
}

/// What this title needs, given its XBE and the symbols the scan found.
pub fn plan(file: &[u8], x: &Xbe, found: &HashMap<String, u32>) -> Plan {
    let mut p = Plan::default();
    let Some(ds) = section(x, "DSOUND") else { return p };
    p.pokes = hrtf(file, x, ds);
    if !x.libraries.iter().any(|l| l.name == "DSOUND" && l.build == 5849) {
        return p;
    }
    let taken: HashMap<u32, &str> = found.iter().filter(|(_, a)| **a != 0).map(|(n, a)| (*a, n.as_str())).collect();
    let inside = |va: u32| va.checked_sub(ds.virtual_address).is_some_and(|o| o < ds.virtual_size);
    for row in DSOUND_5849 {
        let Some(&anchor) = found.get(row.anchor).filter(|a| **a != 0) else {
            p.skipped += 1;
            continue;
        };
        let va = anchor.wrapping_add_signed(row.delta);
        let ok = inside(va)
            && thunk_target(file, x, va).is_some() == row.thunk
            && pops(file, x, va) == Some(row.pops)
            && taken.get(&va).is_none_or(|n| matches!(row.action, Action::Map(m) if m == *n));
        if !ok {
            p.skipped += 1;
            continue;
        }
        match row.action {
            Action::Map(name) => {
                if found.get(name) != Some(&va) {
                    p.symbols.push((name, va));
                }
            }
            action => p.hooks.push(Hook { at: va, what: row.what, pops: row.pops, action }),
        }
    }
    p
}

/// `src/common/craft_audio.cpp`: the sample clock and the HRTF table, started by the runtime's
/// `loaded` hook (image in memory, before its entry point).
pub fn audio_source(pokes: &[(u32, u32)]) -> String {
    let mut s = String::from(
        "// Written by Craft Anything; rewritten on each analysis.\n\
         // The audio processor counts output samples at 48 kHz in XGSCNT (0xFE80200C), which XACT, XPP and DirectSound read for\n\
         // timing. The backend has no audio processor, so its register window is backed by memory and the counter runs from a\n\
         // host thread. The HRTF table is what DirectSoundUse*HRTF would have filled: the 3D maths calls through it.\n\
         #include <chrono>\n#include <cstdint>\n#include <thread>\n\n#include \"kernel/memory.h\"\n\n\
         void CraftAudioStart() {\n",
    );
    for (a, v) in pokes {
        s.push_str(&format!("    *reinterpret_cast<uint32_t*>(mem::Base() + 0x{a:X}u) = 0x{v:X}u;\n"));
    }
    s.push_str(
        "    if (!mem::Commit(0xFE800000u, 0x80000u)) return;\n\
         \x20   std::thread([] {\n\
         \x20       auto* counter = reinterpret_cast<volatile uint32_t*>(mem::Base() + 0xFE80200Cu);\n\
         \x20       const auto start = std::chrono::steady_clock::now();\n\
         \x20       for (;;) {\n\
         \x20           auto us = std::chrono::duration_cast<std::chrono::microseconds>(std::chrono::steady_clock::now() - start).count();\n\
         \x20           *counter = uint32_t(uint64_t(us) * 48000 / 1000000);\n\
         \x20           std::this_thread::sleep_for(std::chrono::milliseconds(1));\n\
         \x20       }\n\
         \x20   }).detach();\n}\n",
    );
    s
}

/// `src/game/craft_dsound.cpp`: the host stand-ins.
pub fn hooks_source(hooks: &[Hook]) -> String {
    let mut s = String::from(
        "// Written by Craft Anything; rewritten on each analysis. DirectSound functions XACT reaches that the backend\n// doesn't place: host stand-ins, so the title's own DirectSound code never runs on the host's objects.\n#include <cmath>\n#include <cstring>\n\n#include \"xdk/hle.h\"\n",
    );
    for h in hooks {
        let body = match h.action {
            Action::Clear(n) => format!("    std::memset(::kernel::HostPtr(::kernel::R32(ctx.esp + 8)), 0, {n});\n    ::kernel::Return(ctx, 0, {});\n", h.pops),
            Action::CreateStream => format!(
                "    uint32_t r = ::kernel::CallGuest(ctx, SYM(DirectSoundCreateStream), {{::kernel::R32(ctx.esp + 8), ::kernel::R32(ctx.esp + 12)}});\n    ::kernel::Return(ctx, r, {});\n",
                h.pops
            ),
            Action::Pitch(set) => format!(
                "    uint32_t hz = uint32_t(48000.0 * std::exp2(int32_t(::kernel::R32(ctx.esp + 8)) / 4096.0) + 0.5);\n    uint32_t r = ::kernel::CallGuest(ctx, SYM({set}), {{::kernel::R32(ctx.esp + 4), hz}});\n    ::kernel::Return(ctx, r, {});\n",
                h.pops
            ),
            Action::Accept | Action::Map(_) => format!("    ::kernel::Return(ctx, 0, {});\n", h.pops),
        };
        s.push_str(&format!("\nX86_FUNC(sub_{:08X}) {{  // {}\n    (void)base;\n{body}}}\n", h.at, h.what));
    }
    s
}

/// Starts [`audio_source`] from the runtime's `loaded` hook in `game_info.cpp`, unless that hook
/// is already taken by something else. None when it can't be wired.
pub fn wire_loaded(game_info: &str) -> Option<String> {
    if game_info.contains(".loaded = CraftAudioStart,") {
        return Some(game_info.to_string());
    }
    let (before, after) = game_info.split_once(".loaded = nullptr,")?;
    let with = format!("{before}.loaded = CraftAudioStart,{after}");
    let (head, tail) = with.split_once("#include \"app/app.h\"\n")?;
    Some(format!("{head}#include \"app/app.h\"\n\nvoid CraftAudioStart();  // craft_audio.cpp\n{tail}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An XBE whose DSOUND section holds `code` at 0x10000, linking DSOUND 1.0.5849.
    fn image(code: &[u8]) -> (Vec<u8>, Xbe) {
        let x = Xbe {
            base: 0x10000,
            image_size: 0x20000,
            title_id: String::new(),
            title_code: String::new(),
            title_name: String::new(),
            regions: Vec::new(),
            disc_number: 0,
            version: 0,
            sections: vec![xbe::Section {
                name: "DSOUND".into(),
                virtual_address: 0x10000,
                virtual_size: code.len() as u32,
                raw_offset: 0x100,
                raw_size: code.len() as u32,
                executable: true,
                writable: false,
            }],
            libraries: vec![xbe::Library { name: "DSOUND".into(), major: 1, minor: 0, build: 5849, qfe: 1, debug: false }],
            headers: 0,
            entry: None,
            kernel_thunk: None,
            keys: None,
        };
        let mut file = vec![0u8; 0x100];
        file.extend_from_slice(code);
        (file, x)
    }

    #[test]
    fn finds_the_hrtf_table_and_skips_lookalikes() {
        let mut code = vec![0xCC; 16];
        // seven writes only: too short to be the table
        for n in 0..7u32 {
            code.extend_from_slice(&[0xC7, 0x05]);
            code.extend_from_slice(&(0x2000 + n * 4).to_le_bytes());
            code.extend_from_slice(&n.to_le_bytes());
        }
        code.push(0xC3);
        let table_at = code.len();
        for n in 0..9u32 {
            code.extend_from_slice(&[0xC7, 0x05]);
            code.extend_from_slice(&(0x3000 + n * 4).to_le_bytes());
            code.extend_from_slice(&(0x10000 + n).to_le_bytes());
        }
        code.push(0xC3);
        code.extend_from_slice(&[0xFF, 0x15, 0x00, 0x30, 0x00, 0x00]); // call [0x3000]
        let (file, x) = image(&code);
        let w = hrtf(&file, &x, &x.sections[0]);
        assert_eq!(w.len(), 9, "{table_at}");
        assert_eq!(w[0], (0x3000, 0x10000));
        // truncated mid-table: nothing, no panic
        let (file, x) = image(&code[..table_at + 15]);
        assert!(hrtf(&file, &x, &x.sections[0]).is_empty());
    }

    #[test]
    fn places_functions_by_anchor_and_checks_their_arguments() {
        // SetVolume at 0x10000, four 0x1C wrappers after it, each `ret 8`; the fourth (SetFilter) pops 3 here.
        let mut code = Vec::new();
        for n in 0..5 {
            let mut w = vec![0x90; 0x1C];
            let ret = if n == 4 { [0xC2, 0x0C, 0x00] } else { [0xC2, 0x08, 0x00] };
            w[0x19..].copy_from_slice(&ret);
            code.extend_from_slice(&w);
        }
        let (file, x) = image(&code);
        let found = HashMap::from([("IDirectSoundBuffer_SetVolume".to_string(), 0x10000), ("IDirectSoundBuffer_SetFilter".to_string(), 0)]);
        let p = plan(&file, &x, &found);
        assert_eq!(
            p.hooks.iter().map(|h| (h.at, h.what)).collect::<Vec<_>>(),
            [(0x1001C, "IDirectSoundBuffer_SetPitch"), (0x10038, "IDirectSoundBuffer_SetLFO")]
        );
        assert_eq!(p.symbols, [("IDirectSoundBuffer_SetEG", 0x10054)]);
        // SetFilter's argument count doesn't match, so it's left alone; rows without anchors are skipped
        assert!(p.skipped > 0 && !p.symbols.iter().any(|s| s.0 == "IDirectSoundBuffer_SetFilter"));
    }

    #[test]
    fn reads_symbols_and_wires_the_hook_once() {
        let s = symbols("[functions]\n0x1 = \"a\"\n[symbols]\nA = 0x3A9E4C\nB = 0\n[data]\nC = 0x5\n");
        assert_eq!(s.get("A"), Some(&0x3A9E4C));
        assert_eq!(s.get("B"), Some(&0));
        assert!(!s.contains_key("C"));
        let info = "#include \"app/app.h\"\n\nconst app::GameInfo app::kGame = {\n    .loaded = nullptr,   // x\n};\n";
        let wired = wire_loaded(info).unwrap();
        assert!(wired.contains(".loaded = CraftAudioStart,") && wired.contains("void CraftAudioStart();"));
        assert_eq!(wire_loaded(&wired).as_deref(), Some(wired.as_str()));
        assert_eq!(wire_loaded("    .loaded = MyHook,\n"), None);
        let src = hooks_source(&[Hook { at: 0x3AA02C, what: "IDirectSoundBuffer_GetVoiceProperties", pops: 2, action: Action::Clear(0x48) }]);
        assert!(src.contains("X86_FUNC(sub_003AA02C)") && src.contains("memset") && src.contains("Return(ctx, 0, 2)"));
        let pitch =
            hooks_source(&[Hook { at: 0x3A9DF8, what: "IDirectSoundBuffer_SetPitch", pops: 2, action: Action::Pitch("IDirectSoundBuffer_SetFrequency") }]);
        assert!(pitch.contains("std::exp2") && pitch.contains("SYM(IDirectSoundBuffer_SetFrequency)") && pitch.contains("#include <cmath>"));
        assert!(audio_source(&[(0x3CD868, 0x3AF96F)]).contains("mem::Base() + 0x3CD868u) = 0x3AF96Fu;"));
    }
}
