//! Deterministic rectangle bin-packing for sprite-sheet assembly.
//!
//! Complement to the *splitting* code in `animation.rs`: where that module
//! cuts strips into frames, this module lays frames out into sheet bins via
//! the `rectangle-pack` crate (volume heuristic, smallest-box container).
//!
//! Contract summary:
//! - Accepted: 1..=`MAX_PACK_RECTS` rects, each 1..=bin dimension.
//! - Rejected: zero-size rects, rects larger than the bin, bins outside
//!   1..=`MAX_BIN_DIMENSION`, empty bin — all as typed `LumenError`.
//! - Determinism: output is sorted by rect id, so identical inputs always
//!   produce identical placements regardless of internal packing order.

use std::collections::BTreeMap;

use rectangle_pack::{
    GroupedRectsToPlace, RectToInsert, TargetBin, contains_smallest_box, pack_rects,
    volume_heuristic,
};

use crate::LumenError;

/// Upper bound on bin width/height (GPU-friendly texture limit).
pub const MAX_BIN_DIMENSION: u32 = 8192;
/// Upper bound on rect count per pack call (bounds work and memory).
pub const MAX_PACK_RECTS: usize = 4096;

/// One rectangle to place. `id` is caller-chosen and echoed back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackRequest {
    pub id: usize,
    pub width: u32,
    pub height: u32,
}

/// Where a rectangle landed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackedPlacement {
    pub id: usize,
    /// Bin index (always 0: this API packs into a single bin).
    pub bin: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Pack rects into one `bin_width` x `bin_height` bin.
///
/// Returns placements sorted by `id`. Fails closed when anything does not
/// fit or any input is invalid — no partial placement is ever returned.
pub fn pack_sprites(
    requests: &[PackRequest],
    bin_width: u32,
    bin_height: u32,
) -> Result<Vec<PackedPlacement>, LumenError> {
    if bin_width == 0 || bin_height == 0 {
        return Err(LumenError::BadParam(
            "bin dimensions must be positive".into(),
        ));
    }
    if bin_width > MAX_BIN_DIMENSION || bin_height > MAX_BIN_DIMENSION {
        return Err(LumenError::BadParam(format!(
            "bin {bin_width}x{bin_height} exceeds {MAX_BIN_DIMENSION}px bound"
        )));
    }
    if requests.len() > MAX_PACK_RECTS {
        return Err(LumenError::BadParam(format!(
            "too many rects: {} > {MAX_PACK_RECTS}",
            requests.len()
        )));
    }
    // Duplicate ids would make output ambiguous; reject up front.
    let mut seen = std::collections::BTreeSet::new();
    for r in requests {
        if r.width == 0 || r.height == 0 {
            return Err(LumenError::BadParam(format!(
                "rect id {} has zero size",
                r.id
            )));
        }
        if r.width > bin_width || r.height > bin_height {
            return Err(LumenError::BadParam(format!(
                "rect id {} ({}x{}) does not fit bin {bin_width}x{bin_height}",
                r.id, r.width, r.height
            )));
        }
        if !seen.insert(r.id) {
            return Err(LumenError::BadParam(format!("duplicate rect id {}", r.id)));
        }
    }

    let mut to_place: GroupedRectsToPlace<usize, ()> = GroupedRectsToPlace::new();
    for r in requests {
        to_place.push_rect(r.id, None, RectToInsert::new(r.width, r.height, 1));
    }
    let mut bins = BTreeMap::new();
    bins.insert(0u32, TargetBin::new(bin_width, bin_height, 1));
    let packed = pack_rects(
        &to_place,
        &mut bins,
        &volume_heuristic,
        &contains_smallest_box,
    )
    .map_err(|e| LumenError::BadParam(format!("packing failed: {e:?}")))?;

    let mut out = Vec::with_capacity(requests.len());
    for (id, (bin, loc)) in packed.packed_locations() {
        out.push(PackedPlacement {
            id: *id,
            bin: *bin,
            x: loc.x(),
            y: loc.y(),
            width: loc.width(),
            height: loc.height(),
        });
    }
    out.sort_by_key(|p| p.id);
    Ok(out)
}

/// Pack rects into the smallest square bin in `[start_bin, max_bin]`.
///
/// `start_bin` doubles until everything fits or `max_bin` is exceeded.
/// Returns placements (sorted by id) and the bin size actually used.
/// Rect-level validation (non-zero, within `max_bin`, id uniqueness) runs
/// once up front; only genuine fit failures trigger growth.
pub fn pack_sprites_grow(
    requests: &[PackRequest],
    start_bin: u32,
    max_bin: u32,
) -> Result<(Vec<PackedPlacement>, u32), LumenError> {
    if start_bin == 0 || start_bin > max_bin {
        return Err(LumenError::BadParam(format!(
            "start_bin {start_bin} must be 1..=max_bin {max_bin}"
        )));
    }
    if max_bin > MAX_BIN_DIMENSION {
        return Err(LumenError::BadParam(format!(
            "max_bin {max_bin} exceeds {MAX_BIN_DIMENSION}px bound"
        )));
    }
    if requests.len() > MAX_PACK_RECTS {
        return Err(LumenError::BadParam(format!(
            "too many rects: {} > {MAX_PACK_RECTS}",
            requests.len()
        )));
    }
    let mut seen = std::collections::BTreeSet::new();
    for r in requests {
        if r.width == 0 || r.height == 0 {
            return Err(LumenError::BadParam(format!(
                "rect id {} has zero size",
                r.id
            )));
        }
        if r.width > max_bin || r.height > max_bin {
            return Err(LumenError::BadParam(format!(
                "rect id {} ({}x{}) exceeds max_bin {max_bin}",
                r.id, r.width, r.height
            )));
        }
        if !seen.insert(r.id) {
            return Err(LumenError::BadParam(format!("duplicate rect id {}", r.id)));
        }
    }

    let mut bin = start_bin;
    loop {
        let mut to_place: GroupedRectsToPlace<usize, ()> = GroupedRectsToPlace::new();
        for r in requests {
            to_place.push_rect(r.id, None, RectToInsert::new(r.width, r.height, 1));
        }
        let mut bins = BTreeMap::new();
        bins.insert(0u32, TargetBin::new(bin, bin, 1));
        match pack_rects(
            &to_place,
            &mut bins,
            &volume_heuristic,
            &contains_smallest_box,
        ) {
            Ok(packed) => {
                let mut out = Vec::with_capacity(requests.len());
                for (id, (bin_id, loc)) in packed.packed_locations() {
                    out.push(PackedPlacement {
                        id: *id,
                        bin: *bin_id,
                        x: loc.x(),
                        y: loc.y(),
                        width: loc.width(),
                        height: loc.height(),
                    });
                }
                out.sort_by_key(|p| p.id);
                return Ok((out, bin));
            }
            Err(_) => {
                let next = bin.saturating_mul(2);
                if next > max_bin || next <= bin {
                    return Err(LumenError::BadParam(format!(
                        "{} rects do not fit in a {max_bin}px bin",
                        requests.len()
                    )));
                }
                bin = next;
            }
        }
    }
}

#[cfg(test)]
mod pack_tests {
    use super::*;

    fn req(id: usize, w: u32, h: u32) -> PackRequest {
        PackRequest {
            id,
            width: w,
            height: h,
        }
    }

    // Validation: four quads tile a bin exactly
    #[test]
    fn four_quads_tile_bin() {
        let rects = vec![
            req(0, 64, 64),
            req(1, 64, 64),
            req(2, 64, 64),
            req(3, 64, 64),
        ];
        let placed = pack_sprites(&rects, 128, 128).unwrap();
        assert_eq!(placed.len(), 4);
        // Sorted by id for determinism
        let ids: Vec<usize> = placed.iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![0, 1, 2, 3]);
        // Every placement inside the bin, no overlaps
        for p in &placed {
            assert!(p.x + p.width <= 128 && p.y + p.height <= 128);
        }
        for (i, a) in placed.iter().enumerate() {
            for b in placed.iter().skip(i + 1) {
                let overlap = a.x < b.x + b.width
                    && b.x < a.x + a.width
                    && a.y < b.y + b.height
                    && b.y < a.y + a.height;
                assert!(!overlap, "overlap between {a:?} and {b:?}");
            }
        }
    }

    // Validation: empty input packs to empty output
    #[test]
    fn empty_packs_empty() {
        assert!(pack_sprites(&[], 256, 256).unwrap().is_empty());
    }

    // Validation: deterministic across runs
    #[test]
    fn deterministic_across_runs() {
        let rects: Vec<PackRequest> = (0..20)
            .map(|i| req(i, 16 + (i as u32 * 7) % 48, 16 + (i as u32 * 13) % 48))
            .collect();
        let a = pack_sprites(&rects, 512, 512).unwrap();
        let b = pack_sprites(&rects, 512, 512).unwrap();
        assert_eq!(a, b);
    }

    // Adversarial: zero-size rect rejected
    #[test]
    fn zero_size_rejected() {
        assert!(pack_sprites(&[req(0, 0, 16)], 128, 128).is_err());
        assert!(pack_sprites(&[req(0, 16, 0)], 128, 128).is_err());
    }

    // Adversarial: oversize rect rejected (not a packer panic)
    #[test]
    fn oversize_rejected() {
        assert!(pack_sprites(&[req(0, 129, 16)], 128, 128).is_err());
        assert!(pack_sprites(&[req(0, 16, 1024)], 128, 128).is_err());
    }

    // Adversarial: zero bin rejected
    #[test]
    fn zero_bin_rejected() {
        assert!(pack_sprites(&[req(0, 16, 16)], 0, 128).is_err());
    }

    // Adversarial: duplicate ids rejected
    #[test]
    fn duplicate_ids_rejected() {
        assert!(pack_sprites(&[req(0, 16, 16), req(0, 16, 16)], 128, 128).is_err());
    }

    // Adversarial: bin over the dimension bound rejected
    #[test]
    fn huge_bin_rejected() {
        assert!(pack_sprites(&[req(0, 16, 16)], MAX_BIN_DIMENSION + 1, 128).is_err());
    }

    // Validation: grow uses the start bin when everything fits
    #[test]
    fn grow_uses_start_bin_when_fits() {
        let rects = vec![req(0, 32, 32)];
        let (placed, bin) = pack_sprites_grow(&rects, 128, 1024).unwrap();
        assert_eq!(bin, 128);
        assert_eq!(placed.len(), 1);
    }

    // Validation: grow doubles until the rect fits
    #[test]
    fn grow_doubles_to_fit() {
        let rects = vec![req(0, 100, 100)];
        let (placed, bin) = pack_sprites_grow(&rects, 64, 1024).unwrap();
        assert!(bin >= 128);
        assert_eq!(placed[0].width, 100);
    }

    // Adversarial: grow fails closed past max_bin
    #[test]
    fn grow_fails_past_max() {
        let rects = vec![req(0, 9000, 9000)];
        assert!(pack_sprites_grow(&rects, 128, 8192).is_err());
    }

    // Adversarial: grow rejects bad start_bin
    #[test]
    fn grow_bad_start_rejected() {
        let rects = vec![req(0, 16, 16)];
        assert!(pack_sprites_grow(&rects, 0, 1024).is_err());
        assert!(pack_sprites_grow(&rects, 2048, 1024).is_err());
    }
}
