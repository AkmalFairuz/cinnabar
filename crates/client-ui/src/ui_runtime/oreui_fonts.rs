//! Shipped compiled faces used by OreUI's semantic text styles.

use std::{io::Read, path::Path, sync::Arc};

use assets::RuntimeFontCatalog;

use ui::oreui_theme::OreUiFont;

/// Attaches valid optional carriers, retaining the default font for unavailable faces.
pub fn install(base: Arc<RuntimeFontCatalog>, compiled_dir: &Path) -> Arc<RuntimeFontCatalog> {
    install_from(base, compiled_dir, assets::carriers::file::open)
}

/// [`install`] with carriers opened by `open`, as a browser's preloaded ones are.
fn install_from(
    base: Arc<RuntimeFontCatalog>,
    compiled_dir: &Path,
    open: impl Fn(&Path) -> std::io::Result<assets::carriers::file::CarrierFile>,
) -> Arc<RuntimeFontCatalog> {
    let mut combined = (*base).clone();
    for face in OreUiFont::ALL {
        let path = compiled_dir.join(face.carrier().output);
        match open(&path)
            .map_err(|e| e.to_string())
            .and_then(|file| load(face, file))
            .and_then(|font| {
                combined
                    .with_named_font(face.name(), &font)
                    .map_err(|e| e.to_string())
            }) {
            Ok(font) => combined = font,
            Err(reason) => eprintln!(
                "{} unavailable at {} ({reason}); using default font; rebuild with make {}-assets",
                face.name(),
                path.display(),
                face.carrier().name
            ),
        }
    }
    if combined.identity() == base.identity() {
        base
    } else {
        Arc::new(combined)
    }
}

/// Bounds the read and validates both the carrier payload and its pinned source manifest.
fn load(face: OreUiFont, file: impl Read) -> Result<RuntimeFontCatalog, String> {
    let mut bytes = Vec::new();
    file.take(assets::MAX_FONT_CARRIER_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > assets::MAX_FONT_CARRIER_BYTES {
        return Err("font carrier exceeds its byte bound".into());
    }
    let profile = face.carrier().font_face.unwrap();
    let manifest = assets::canonical_source_manifest_sha256(profile.manifest);
    RuntimeFontCatalog::decode(&bytes, manifest)
        .and_then(|font| font.with_line_metrics(profile.line_metrics()))
        .map(|font| font.with_coverage_pages())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;
