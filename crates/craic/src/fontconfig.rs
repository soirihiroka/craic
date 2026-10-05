use fontconfig_sys::{
    FcConfigDestroy, FcConfigFilename, FcConfigGetFonts, FcInitLoadConfigAndFonts,
    FcPatternGetString, FcResultMatch, FcSetApplication, FcSetSystem, FcStrFree,
};
use std::collections::BTreeSet;
use std::ffi::CStr;
use std::io::{self, Write};
use std::ptr;
use tempfile::NamedTempFile;

/// Prepare an application-private Fontconfig configuration before any threads start.
/// Skia's family enumeration loops forever when a pattern has no family property:
/// Fontconfig returns NoMatch, but Skia only stops on NoId. Other font consumers
/// handle these entries correctly, so leave the user's configuration untouched.
pub fn prepare() -> io::Result<Option<(NamedTempFile, usize)>> {
    let mut excluded = BTreeSet::new();
    // SAFETY: Fontconfig owns the sets and patterns until FcConfigDestroy. All
    // borrowed strings are copied before releasing the configuration.
    let source = unsafe {
        let config = FcInitLoadConfigAndFonts();
        if config.is_null() {
            return Err(io::Error::other("Fontconfig initialization failed"));
        }
        for set_name in [FcSetSystem, FcSetApplication] {
            let set = FcConfigGetFonts(config, set_name);
            if set.is_null() {
                continue;
            }
            let set = &*set;
            for index in 0..set.nfont as usize {
                let pattern = *set.fonts.add(index);
                let mut family = ptr::null_mut();
                if FcPatternGetString(pattern, c"family".as_ptr(), 0, &mut family) == FcResultMatch
                {
                    continue;
                }
                let mut file = ptr::null_mut();
                if FcPatternGetString(pattern, c"file".as_ptr(), 0, &mut file) == FcResultMatch
                    && !file.is_null()
                {
                    excluded.insert(CStr::from_ptr(file.cast()).to_bytes().to_vec());
                }
            }
        }
        let filename = FcConfigFilename(ptr::null());
        let source = if filename.is_null() {
            None
        } else {
            let source = CStr::from_ptr(filename.cast()).to_bytes().to_vec();
            FcStrFree(filename);
            Some(source)
        };
        FcConfigDestroy(config);
        source
    };
    if excluded.is_empty() {
        return Ok(None);
    }
    let source = source.ok_or_else(|| io::Error::other("Fontconfig configuration not found"))?;
    let mut configuration = NamedTempFile::new()?;
    writeln!(configuration, "<?xml version=\"1.0\"?><fontconfig>")?;
    // Preserve the existing configuration, including user rules and font fallback.
    // Exact file patterns avoid treating wildcard characters in paths as globs.
    for (tag, paths) in [
        ("include", vec![source]),
        ("rejectfont", excluded.iter().cloned().collect()),
    ] {
        if tag == "rejectfont" {
            writeln!(configuration, "<selectfont><rejectfont>")?;
        }
        for path in paths {
            let path = std::str::from_utf8(&path)
                .map_err(|_| io::Error::other("Fontconfig font path is not UTF-8"))?
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('\"', "&quot;")
                .replace('\'', "&apos;");
            if tag == "include" {
                writeln!(configuration, "<include>{path}</include>")?;
            } else {
                writeln!(
                    configuration,
                    "<pattern><patelt name=\"file\"><string>{path}</string></patelt></pattern>"
                )?;
            }
        }
        if tag == "rejectfont" {
            writeln!(configuration, "</rejectfont></selectfont>")?;
        }
    }
    writeln!(configuration, "</fontconfig>")?;
    configuration.flush()?;
    // SAFETY: The only caller runs at process startup, before GTK or workers can
    // read the environment. The caller restores it after retaining the font manager.
    unsafe { std::env::set_var("FONTCONFIG_FILE", configuration.path()) };
    Ok(Some((configuration, excluded.len())))
}
