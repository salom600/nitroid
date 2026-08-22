//! Binary translation bridge — lets ARM Android binaries run on x86_64 hosts.
//!
//! ## Design
//!
//! Writing an AOT/JIT ARM-to-x86_64 translator from scratch is a multi-engineer
//! effort. Nitroid deliberately avoids reinventing it. Instead, this crate
//! exposes a clean [`Translator`] trait that wraps a pluggable translation
//! backend:
//!
//! - `Houdini` — the closed-source translation layer used by some Android
//!   x86 distributions (loaded dynamically if present in the system image).
//! - `Libhoudini64` — alternative, often shipped with Android-x86 images.
//! - `Rosetta`-style bridge — when running on Apple Silicon (future work).
//! - `Native` — no translation; the guest is already x86_64.
//!
//! All backends implement the same trait, so the rest of the codebase never
//! needs to know which one is in use.
//!
//! ## Why not build the translator in Rust?
//!
//! A correct ARMv8.5-A → x86_64 translator needs to handle:
//!
//! - conditional flags, flagless arithmetic, FPCR trapping
//! - SVE/SVE2 vector operations (which have no direct x86 equivalent)
//! - NEON → AVX/AVX2/AVX-512 mapping (with proper NaN boxing)
//! - ASID/TLB management, atomic ordering semantics
//! - exclusive monitor / LDXR/STXR sequence correctness
//!
//! Google's own translator took years. We integrate, we don't rebuild.

pub mod backend;
pub mod cache;

pub use backend::{NativeBackend, Translator, TranslatorBackend};
pub use cache::TranslationCache;

use nitroid_core::{CpuArch, Result};

/// Pick the right translator backend for the given guest arch and host arch.
pub fn pick_backend(guest: CpuArch) -> Result<TranslatorBackend> {
    #[cfg(target_arch = "x86_64")]
    {
        return match guest {
            CpuArch::X86_64 => Ok(TranslatorBackend::Native(NativeBackend)),
            CpuArch::Aarch64 | CpuArch::Armv7 => {
                // Probe for a system-image-bundled translator first.
                if backend::probe_houdini().is_some() {
                    Ok(TranslatorBackend::Houdini)
                } else {
                    Ok(TranslatorBackend::Unavailable)
                }
            }
        };
    }
    #[cfg(target_arch = "aarch64")]
    {
        return match guest {
            CpuArch::Aarch64 | CpuArch::Armv7 => Ok(TranslatorBackend::Native(NativeBackend)),
            CpuArch::X86_64 => Ok(TranslatorBackend::Unavailable),
        };
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        let _ = guest;
        Ok(TranslatorBackend::Unavailable)
    }
}
