//! MMD mapping profile as data (SPEC §6) plus the D-631
//! MMD-edition proportion profile and the D-630 `Fac_` face-channel
//! vocabulary.
//!
//! The tables in this module are *data*: Stage 2's
//! `crates/torogao-format/data/mmd-profile/1.ron` mirrors these
//! structures, so a mapping correction becomes a data amendment in
//! both places, never a logic change. Census records stay verbatim
//! — mappings live here and in GLB extras, never in the census.
//!
//! Proportion numbers come only from DOSSIER §6 measurements
//! (`~/workspace/zenless-models-20261102/DOSSIER.md`): total height
//! band 17.77–23.19 PMX units, heroine class 19.4–21.9, head-bone
//! fraction 0.79–0.89 (median 0.87), scale 0.08 (SPEC §5.1).

use super::parse::{MorphOffset, PmxModel};

/// One bone-mapping row: PMX local name → torogao-humanoid/1 (VMC).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoneMapping {
    /// PMX local (Japanese) name.
    pub pmx_local: &'static str,
    /// English gloss.
    pub gloss: &'static str,
    /// torogao-humanoid/1 target, or "" when ledgered unmapped.
    pub torogao: &'static str,
}

/// One morph-mapping row: PMX local name → governed channel (§6.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MorphMapping {
    /// PMX local (Japanese) name.
    pub pmx_local: &'static str,
    /// English gloss.
    pub gloss: &'static str,
    /// Torogao governed channel, or "" when ledgered unmapped.
    pub torogao_channel: &'static str,
}

/// The §6.1 bone table (left/right pairs expanded; aliases included).
pub const BONE_MAPPINGS: &[BoneMapping] = &[
    BoneMapping {
        pmx_local: "全ての親",
        gloss: "all parents",
        torogao: "",
    },
    BoneMapping {
        pmx_local: "センター",
        gloss: "center",
        torogao: "Hips",
    },
    BoneMapping {
        pmx_local: "グルーブ",
        gloss: "groove",
        torogao: "Hips",
    },
    BoneMapping {
        pmx_local: "腰",
        gloss: "waist",
        torogao: "Hips",
    },
    BoneMapping {
        pmx_local: "下半身",
        gloss: "lower body",
        torogao: "Spine",
    },
    BoneMapping {
        pmx_local: "上半身",
        gloss: "upper body",
        torogao: "Spine",
    },
    BoneMapping {
        pmx_local: "上半身2",
        gloss: "upper body 2",
        torogao: "Chest",
    },
    BoneMapping {
        pmx_local: "首",
        gloss: "neck",
        torogao: "Neck",
    },
    BoneMapping {
        pmx_local: "頭",
        gloss: "head",
        torogao: "Head",
    },
    BoneMapping {
        pmx_local: "左肩",
        gloss: "left shoulder",
        torogao: "LeftShoulder",
    },
    BoneMapping {
        pmx_local: "右肩",
        gloss: "right shoulder",
        torogao: "RightShoulder",
    },
    BoneMapping {
        pmx_local: "左腕",
        gloss: "left arm",
        torogao: "LeftUpperArm",
    },
    BoneMapping {
        pmx_local: "右腕",
        gloss: "right arm",
        torogao: "RightUpperArm",
    },
    BoneMapping {
        pmx_local: "左ひじ",
        gloss: "left elbow",
        torogao: "LeftLowerArm",
    },
    BoneMapping {
        pmx_local: "右ひじ",
        gloss: "right elbow",
        torogao: "RightLowerArm",
    },
    BoneMapping {
        pmx_local: "左肘",
        gloss: "left elbow (alias)",
        torogao: "LeftLowerArm",
    },
    BoneMapping {
        pmx_local: "右肘",
        gloss: "right elbow (alias)",
        torogao: "RightLowerArm",
    },
    BoneMapping {
        pmx_local: "左手首",
        gloss: "left wrist",
        torogao: "LeftHand",
    },
    BoneMapping {
        pmx_local: "右手首",
        gloss: "right wrist",
        torogao: "RightHand",
    },
    BoneMapping {
        pmx_local: "左手先",
        gloss: "left hand tip",
        torogao: "",
    },
    BoneMapping {
        pmx_local: "右手先",
        gloss: "right hand tip",
        torogao: "",
    },
    BoneMapping {
        pmx_local: "左足",
        gloss: "left leg",
        torogao: "LeftUpperLeg",
    },
    BoneMapping {
        pmx_local: "右足",
        gloss: "right leg",
        torogao: "RightUpperLeg",
    },
    BoneMapping {
        pmx_local: "左ひざ",
        gloss: "left knee",
        torogao: "LeftLowerLeg",
    },
    BoneMapping {
        pmx_local: "右ひざ",
        gloss: "right knee",
        torogao: "RightLowerLeg",
    },
    BoneMapping {
        pmx_local: "左膝",
        gloss: "left knee (alias)",
        torogao: "LeftLowerLeg",
    },
    BoneMapping {
        pmx_local: "右膝",
        gloss: "right knee (alias)",
        torogao: "RightLowerLeg",
    },
    BoneMapping {
        pmx_local: "左足首",
        gloss: "left ankle",
        torogao: "LeftFoot",
    },
    BoneMapping {
        pmx_local: "右足首",
        gloss: "right ankle",
        torogao: "RightFoot",
    },
    BoneMapping {
        pmx_local: "左つま先",
        gloss: "left toe",
        torogao: "LeftToes",
    },
    BoneMapping {
        pmx_local: "右つま先",
        gloss: "right toe",
        torogao: "RightToes",
    },
    BoneMapping {
        pmx_local: "左目",
        gloss: "left eye (gaze)",
        torogao: "",
    },
    BoneMapping {
        pmx_local: "右目",
        gloss: "right eye (gaze)",
        torogao: "",
    },
];

/// Look up a PMX bone's torogao-humanoid/1 target by local name.
/// Returns `None` when the profile ledgers the bone as unmapped
/// (including names absent from the table).
pub fn bone_target(pmx_local: &str) -> Option<&'static str> {
    BONE_MAPPINGS
        .iter()
        .find(|row| row.pmx_local == pmx_local)
        .and_then(|row| {
            if row.torogao.is_empty() {
                None
            } else {
                Some(row.torogao)
            }
        })
}

/// The §6.2 morph table (PMX standard names → governed channels).
pub const MORPH_MAPPINGS: &[MorphMapping] = &[
    MorphMapping {
        pmx_local: "まばたき",
        gloss: "blink",
        torogao_channel: "blink",
    },
    MorphMapping {
        pmx_local: "笑い",
        gloss: "smile eyes",
        torogao_channel: "happyEye",
    },
    MorphMapping {
        pmx_local: "ウィンク",
        gloss: "wink (left)",
        torogao_channel: "blinkLeft",
    },
    MorphMapping {
        pmx_local: "ウィンク右",
        gloss: "wink right",
        torogao_channel: "blinkRight",
    },
    MorphMapping {
        pmx_local: "ウィンク２",
        gloss: "wink 2 (left)",
        torogao_channel: "blinkLeft",
    },
    MorphMapping {
        pmx_local: "ウィンク右２",
        gloss: "wink 2 right",
        torogao_channel: "blinkRight",
    },
    MorphMapping {
        pmx_local: "瞳小",
        gloss: "pupils small",
        torogao_channel: "pupilScale",
    },
    MorphMapping {
        pmx_local: "あ",
        gloss: "viseme a",
        torogao_channel: "aa",
    },
    MorphMapping {
        pmx_local: "い",
        gloss: "viseme i",
        torogao_channel: "ih",
    },
    MorphMapping {
        pmx_local: "う",
        gloss: "viseme u",
        torogao_channel: "ou",
    },
    MorphMapping {
        pmx_local: "え",
        gloss: "viseme e",
        torogao_channel: "ee",
    },
    MorphMapping {
        pmx_local: "お",
        gloss: "viseme o",
        torogao_channel: "oh",
    },
    MorphMapping {
        pmx_local: "怒り",
        gloss: "anger brows",
        torogao_channel: "browAnger",
    },
    MorphMapping {
        pmx_local: "困る",
        gloss: "troubled brows",
        torogao_channel: "browTrouble",
    },
    MorphMapping {
        pmx_local: "眉上",
        gloss: "brows up",
        torogao_channel: "browRaise",
    },
    MorphMapping {
        pmx_local: "眉下",
        gloss: "brows down",
        torogao_channel: "browLower",
    },
    MorphMapping {
        pmx_local: "にやり",
        gloss: "smirk",
        torogao_channel: "mouthSmirk",
    },
    MorphMapping {
        pmx_local: "びっくり",
        gloss: "surprised",
        torogao_channel: "surprise",
    },
    MorphMapping {
        pmx_local: "真面目",
        gloss: "serious",
        torogao_channel: "serious",
    },
    MorphMapping {
        pmx_local: "口角上げ",
        gloss: "mouth corners up",
        torogao_channel: "mouthCornerUp",
    },
    MorphMapping {
        pmx_local: "口角下げ",
        gloss: "mouth corners down",
        torogao_channel: "mouthCornerDown",
    },
];

/// Look up a PMX morph's governed channel by local name.
pub fn morph_channel(pmx_local: &str) -> Option<&'static str> {
    MORPH_MAPPINGS
        .iter()
        .find(|row| row.pmx_local == pmx_local)
        .map(|row| row.torogao_channel)
}

/// The D-630 canonical `Fac_` face-channel vocabulary (DOSSIER §4).
/// Lid channels beyond these are reserved and out of scope: no
/// names are invented here.
pub const FAC_CHANNELS: &[&str] = &[
    "Fac_Eye_Close",
    "Fac_Eye_L_Open",
    "Fac_Eye_R_Open",
    "Fac_Eye_L_Wink",
    "Fac_Eye_R_Wink",
    "Fac_Eye_HalfClose",
    "Fac_Eye_Sad",
    "Fac_Eye_Angry",
    "Fac_Eye_MidUp",
    "Fac_Eye_MidDown",
    "Fac_Eye_LowlidUp",
    "Fac_Mth_Aa",
    "Fac_Mth_Ii",
    "Fac_Mth_Uu",
    "Fac_Mth_Ee",
    "Fac_Mth_Oo",
    "Fac_Mth_L_Up",
    "Fac_Mth_R_Up",
    "Fac_Mth_L_Down",
    "Fac_Mth_R_Down",
    "Fac_Mth_L_In",
    "Fac_Mth_R_In",
    "Fac_Mth_L_Out",
    "Fac_Mth_R_Out",
    "Fac_Ebr_Up",
    "Fac_Ebr_Down",
    "Fac_Ebr_Angry",
    "Fac_Ebr_Sad",
    "Fac_Ebr_Relax",
];

/// PMX-standard / English morph names → canonical `Fac_` channel.
/// Only names with a DOSSIER §4 / SPEC §6.2 basis appear here.
const FAC_NAME_MAPPINGS: &[(&str, &str)] = &[
    ("まばたき", "Fac_Eye_Close"),
    ("blink", "Fac_Eye_Close"),
    ("ウィンク", "Fac_Eye_L_Wink"),
    ("ウィンク２", "Fac_Eye_L_Wink"),
    ("ウィンク右", "Fac_Eye_R_Wink"),
    ("ウィンク右２", "Fac_Eye_R_Wink"),
    ("あ", "Fac_Mth_Aa"),
    ("a", "Fac_Mth_Aa"),
    ("い", "Fac_Mth_Ii"),
    ("i", "Fac_Mth_Ii"),
    ("う", "Fac_Mth_Uu"),
    ("u", "Fac_Mth_Uu"),
    ("え", "Fac_Mth_Ee"),
    ("e", "Fac_Mth_Ee"),
    ("お", "Fac_Mth_Oo"),
    ("o", "Fac_Mth_Oo"),
    ("怒り", "Fac_Ebr_Angry"),
    ("眉上", "Fac_Ebr_Up"),
    ("眉下", "Fac_Ebr_Down"),
    ("困る", "Fac_Ebr_Sad"),
];

/// Map one morph to its canonical `Fac_` channel (D-630), checking
/// the universal name first (the ZZZ corpus carries `Fac_` names
/// there), then the local name, then the standard-name table.
/// `None` means explicitly unmapped — callers must ledger it.
pub fn fac_channel(name_local: &str, name_universal: &str) -> Option<&'static str> {
    for candidate in [name_universal, name_local] {
        if let Some(found) = FAC_CHANNELS.iter().find(|ch| **ch == candidate) {
            return Some(found);
        }
    }
    for candidate in [name_local, name_universal] {
        if let Some((_, channel)) = FAC_NAME_MAPPINGS
            .iter()
            .find(|(name, _)| *name == candidate)
        {
            return Some(channel);
        }
    }
    None
}

/// The MMD mapping profile as data: the loader shape Stage 2's RON
/// mirrors. Stage 1 constructs it from the compiled-in tables;
/// the shape (named tables + scale) is the contract.
#[derive(Debug, Clone, PartialEq)]
pub struct MmdProfile {
    /// Profile id.
    pub id: &'static str,
    /// Bone table (§6.1).
    pub bones: &'static [BoneMapping],
    /// Morph table (§6.2).
    pub morphs: &'static [MorphMapping],
    /// Canonical Fac_ channels (D-630).
    pub fac_channels: &'static [&'static str],
    /// Proportion profile (D-631).
    pub proportions: ProportionProfile,
}

impl MmdProfile {
    /// The built-in `mmd-profile/1`.
    pub fn builtin() -> Self {
        MmdProfile {
            id: "mmd-profile/1",
            bones: BONE_MAPPINGS,
            morphs: MORPH_MAPPINGS,
            fac_channels: FAC_CHANNELS,
            proportions: ProportionProfile::mmd_edition(),
        }
    }
}

/// The D-631 MMD-edition proportion profile. Every number is a
/// DOSSIER §6 measurement; nothing is extrapolated.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProportionProfile {
    /// Total height band minimum, PMX units (17.77).
    pub height_min: f32,
    /// Total height band maximum, PMX units (23.19).
    pub height_max: f32,
    /// Heroine-class height band minimum, PMX units (19.4).
    pub heroine_height_min: f32,
    /// Heroine-class height band maximum, PMX units (21.9).
    pub heroine_height_max: f32,
    /// Head-bone fraction band minimum (0.79).
    pub head_fraction_min: f32,
    /// Head-bone fraction band maximum (0.89).
    pub head_fraction_max: f32,
    /// Head-bone fraction median (0.87).
    pub head_fraction_median: f32,
    /// PMX units → metres scale (SPEC §5.1 default 0.08).
    pub scale_to_meters: f32,
}

impl ProportionProfile {
    /// The registered MMD-edition profile (D-631).
    pub fn mmd_edition() -> Self {
        ProportionProfile {
            height_min: 17.77,
            height_max: 23.19,
            heroine_height_min: 19.4,
            heroine_height_max: 21.9,
            head_fraction_min: 0.79,
            head_fraction_max: 0.89,
            head_fraction_median: 0.87,
            scale_to_meters: 0.08,
        }
    }
}

/// A model's measured proportions against the D-631 profile.
/// Height is the bone-position Y range (min..max over bones);
/// the head fraction is the head bone's height above the minimum,
/// divided by the total height. Both are PMX units, unscaled.
#[derive(Debug, Clone, PartialEq)]
pub struct ProportionAssessment {
    /// Measured total height in PMX units (0 when no bones).
    pub height_pmx: f32,
    /// Head-bone fraction of total height, when a head bone exists.
    pub head_fraction: Option<f32>,
    /// True when height is inside the 17.77–23.19 band.
    pub height_in_band: bool,
    /// True when height is inside the heroine 19.4–21.9 band.
    pub height_in_heroine_band: bool,
    /// True when the head fraction is inside 0.79–0.89.
    pub head_fraction_in_band: bool,
}

impl ProportionAssessment {
    /// Deterministic JSON sidecar form (fixed field order).
    pub fn to_json(&self, profile: &ProportionProfile) -> String {
        let head = match self.head_fraction {
            Some(value) => format!("{value}"),
            None => "null".to_string(),
        };
        format!(
            "{{\"profile\": \"mmd-edition/1\", \"scale_to_meters\": {}, \
             \"height_pmx\": {}, \"head_fraction\": {head}, \
             \"height_in_band\": {}, \"height_in_heroine_band\": {}, \
             \"head_fraction_in_band\": {}}}",
            profile.scale_to_meters,
            self.height_pmx,
            self.height_in_band,
            self.height_in_heroine_band,
            self.head_fraction_in_band
        )
    }
}

/// Assess a model against the D-631 proportion profile.
pub fn assess_proportions(model: &PmxModel, profile: &ProportionProfile) -> ProportionAssessment {
    if model.bones.is_empty() {
        return ProportionAssessment {
            height_pmx: 0.0,
            head_fraction: None,
            height_in_band: false,
            height_in_heroine_band: false,
            head_fraction_in_band: false,
        };
    }
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for bone in &model.bones {
        min_y = min_y.min(bone.position[1]);
        max_y = max_y.max(bone.position[1]);
    }
    let height = max_y - min_y;
    let head = model.bones.iter().find(|bone| {
        bone.name_local == "頭"
            || bone.name_universal == "head"
            || bone.name_universal == "Head"
            || bone.name_universal.ends_with(" Head")
    });
    let head_fraction = head.and_then(|bone| {
        if height > 0.0 {
            Some((bone.position[1] - min_y) / height)
        } else {
            None
        }
    });
    ProportionAssessment {
        height_pmx: height,
        head_fraction,
        height_in_band: (profile.height_min..=profile.height_max).contains(&height),
        height_in_heroine_band: (profile.heroine_height_min..=profile.heroine_height_max)
            .contains(&height),
        head_fraction_in_band: head_fraction.is_some_and(|value| {
            (profile.head_fraction_min..=profile.head_fraction_max).contains(&value)
        }),
    }
}

/// One group/flip composition recipe (SPEC §6.3): a named morph
/// driving other morphs at fixed influences.
#[derive(Debug, Clone, PartialEq)]
pub struct MorphRecipe {
    /// Recipe (group/flip morph) name: universal when present, else local.
    pub name: String,
    /// (target morph name, influence) pairs in file order.
    pub drives: Vec<(String, f32)>,
    /// True when every driven morph exists in the model. A recipe
    /// with a missing member is ledgered whole, never half-applied.
    pub complete: bool,
}

/// Build the group/flip recipe sidecar for a model.
pub fn morph_recipes(model: &PmxModel) -> Vec<MorphRecipe> {
    let mut recipes = Vec::new();
    for morph in &model.morphs {
        if morph.morph_type != 0 && morph.morph_type != 9 {
            continue;
        }
        let mut drives = Vec::new();
        let mut complete = true;
        for offset in &morph.offsets {
            if let MorphOffset::Group {
                morph: target,
                influence,
            } = offset
            {
                if *target >= 0 && (*target as usize) < model.morphs.len() {
                    let target_morph = &model.morphs[*target as usize];
                    let name = if target_morph.name_universal.is_empty() {
                        target_morph.name_local.clone()
                    } else {
                        target_morph.name_universal.clone()
                    };
                    drives.push((name, *influence));
                } else {
                    complete = false;
                }
            }
        }
        let name = if morph.name_universal.is_empty() {
            morph.name_local.clone()
        } else {
            morph.name_universal.clone()
        };
        recipes.push(MorphRecipe {
            name,
            drives,
            complete,
        });
    }
    recipes
}

#[cfg(test)]
mod tests {
    use super::{FAC_CHANNELS, fac_channel};

    #[test]
    fn fac_vocabulary_has_no_invented_lid_channels() {
        assert_eq!(FAC_CHANNELS.len(), 29);
        assert!(FAC_CHANNELS.contains(&"Fac_Eye_Close"));
        assert!(FAC_CHANNELS.contains(&"Fac_Mth_Aa"));
        assert!(FAC_CHANNELS.contains(&"Fac_Ebr_Relax"));
    }

    #[test]
    fn fac_mapping_prefers_verbatim_fac_names() {
        assert_eq!(
            fac_channel("何か", "Fac_Eye_HalfClose"),
            Some("Fac_Eye_HalfClose")
        );
        assert_eq!(fac_channel("まばたき", "blink"), Some("Fac_Eye_Close"));
        assert_eq!(fac_channel("あ", "a"), Some("Fac_Mth_Aa"));
        assert_eq!(fac_channel("謎", "mystery"), None);
    }
}
