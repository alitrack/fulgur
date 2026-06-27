//! Conversion functions from Stylo types to Parley types
use std::borrow::Cow;

use style::values::computed::Length;

use crate::node::TextBrush;

// Module of type aliases so we can refer to stylo types with nicer names
pub(crate) mod stylo {
    pub(crate) use style::computed_values::white_space_collapse::T as WhiteSpaceCollapse;
    pub(crate) use style::properties::ComputedValues;
    pub(crate) use style::values::computed::OverflowWrap;
    pub(crate) use style::values::computed::WordBreak;
    pub(crate) use style::values::computed::font::FontStretch;
    pub(crate) use style::values::computed::font::FontStyle;
    pub(crate) use style::values::computed::font::FontVariationSettings;
    pub(crate) use style::values::computed::font::FontWeight;
    pub(crate) use style::values::computed::font::GenericFontFamily;
    pub(crate) use style::values::computed::font::LineHeight;
    pub(crate) use style::values::computed::font::SingleFontFamily;
}

pub(crate) mod parley {
    pub(crate) use parley::FontVariation;
    pub(crate) use parley::fontique::QueryFamily;
    pub(crate) use parley::style::*;
}

pub(crate) fn generic_font_family(input: stylo::GenericFontFamily) -> parley::GenericFamily {
    match input {
        stylo::GenericFontFamily::None => parley::GenericFamily::SansSerif,
        stylo::GenericFontFamily::Serif => parley::GenericFamily::Serif,
        stylo::GenericFontFamily::SansSerif => parley::GenericFamily::SansSerif,
        stylo::GenericFontFamily::Monospace => parley::GenericFamily::Monospace,
        stylo::GenericFontFamily::Cursive => parley::GenericFamily::Cursive,
        stylo::GenericFontFamily::Fantasy => parley::GenericFamily::Fantasy,
        stylo::GenericFontFamily::SystemUi => parley::GenericFamily::SystemUi,
    }
}

#[allow(dead_code)]
pub(crate) fn query_font_family(input: &stylo::SingleFontFamily) -> parley::QueryFamily<'_> {
    match input {
        stylo::SingleFontFamily::FamilyName(name) => {
            'ret: {
                let name = name.name.as_ref();

                // Legacy web compatibility
                #[cfg(target_vendor = "apple")]
                if name == "-apple-system" {
                    break 'ret parley::QueryFamily::Generic(parley::GenericFamily::SystemUi);
                }
                #[cfg(target_os = "macos")]
                if name == "BlinkMacSystemFont" {
                    break 'ret parley::QueryFamily::Generic(parley::GenericFamily::SystemUi);
                }

                break 'ret parley::QueryFamily::Named(name);
            }
        }
        stylo::SingleFontFamily::Generic(generic) => {
            parley::QueryFamily::Generic(self::generic_font_family(*generic))
        }
    }
}

pub(crate) fn font_weight(input: stylo::FontWeight) -> parley::FontWeight {
    parley::FontWeight::new(input.value())
}

pub(crate) fn font_width(input: stylo::FontStretch) -> parley::FontWidth {
    parley::FontWidth::from_percentage(input.0.to_float())
}

pub(crate) fn font_style(input: stylo::FontStyle) -> parley::FontStyle {
    match input {
        stylo::FontStyle::NORMAL => parley::FontStyle::Normal,
        stylo::FontStyle::ITALIC => parley::FontStyle::Italic,
        val => parley::FontStyle::Oblique(Some(val.oblique_degrees())),
    }
}

pub(crate) fn font_variations(input: &stylo::FontVariationSettings) -> Vec<parley::FontVariation> {
    input
        .0
        .iter()
        .map(|v| parley::FontVariation {
            tag: v.tag.0,
            value: v.value,
        })
        .collect()
}

pub(crate) fn white_space_collapse(input: stylo::WhiteSpaceCollapse) -> parley::WhiteSpaceCollapse {
    match input {
        stylo::WhiteSpaceCollapse::Collapse => parley::WhiteSpaceCollapse::Collapse,
        stylo::WhiteSpaceCollapse::Preserve => parley::WhiteSpaceCollapse::Preserve,

        // TODO: Implement PreserveBreaks and BreakSpaces modes
        stylo::WhiteSpaceCollapse::PreserveBreaks => parley::WhiteSpaceCollapse::Preserve,
        stylo::WhiteSpaceCollapse::BreakSpaces => parley::WhiteSpaceCollapse::Preserve,
    }
}

/// Known CJK font family name prefixes. Fonts matching these are presumed
/// to be Pan-CJK fonts whose Latin glyphs are full-width.
const CJK_FAMILY_PREFIXES: &[&str] = &[
    "Noto Sans CJK", "Noto Serif CJK",
    "Source Han Sans", "Source Han Serif",
    "WenQuanYi", "WenQuanYi Micro Hei",
    "PingFang", "Hiragino",
    "Microsoft YaHei", "Microsoft JhengHei",
    "SimSun", "SimHei", "NSimSun", "FangSong", "KaiTi",
    "MS Gothic", "MS Mincho", "MS PGothic", "MS PMincho",
    "Malgun Gothic", "Gulim", "Dotum", "Batang",
];

/// Known Latin font family names that should take priority over CJK fonts.
const LATIN_FAMILY_NAMES: &[&str] = &[
    "DejaVu Sans", "DejaVu Serif", "DejaVu Sans Mono",
    "Liberation Sans", "Liberation Serif", "Liberation Mono",
    "Arial", "Helvetica", "Times New Roman", "Times",
    "Courier New", "Courier", "Verdana", "Georgia",
    "Trebuchet MS", "Tahoma", "Impact", "Comic Sans MS",
];

fn is_cjk_family(name: &str) -> bool {
    CJK_FAMILY_PREFIXES.iter().any(|p| name.starts_with(p))
}

fn is_latin_family(name: &str) -> bool {
    LATIN_FAMILY_NAMES.iter().any(|&n| name.eq_ignore_ascii_case(n))
}

/// Reorder a font-family stack so that Latin fonts take priority over
/// CJK fonts. When a CJK font (e.g., Noto Sans CJK SC) appears before
/// a Latin font (e.g., DejaVu Sans), the Latin font is moved before
/// the first CJK font. This prevents CJK fonts from supplying full-width
/// Latin glyphs when a proportional Latin alternative is available.
fn reorder_cjk_latin(
    mut families: Vec<parley::FontFamily<'static>>,
) -> Vec<parley::FontFamily<'static>> {
    // Find the first CJK-named family and first Latin-named family
    let first_cjk = families.iter().position(|f| {
        matches!(f, parley::FontFamily::Named(name) if is_cjk_family(name))
    });
    let first_latin_after_cjk = first_cjk.and_then(|cjk_idx| {
        families.iter().skip(cjk_idx + 1).position(|f| {
            matches!(f, parley::FontFamily::Named(name) if is_latin_family(name))
        }).map(|offset| cjk_idx + 1 + offset)
    });

    if let (Some(cjk_idx), Some(latin_idx)) = (first_cjk, first_latin_after_cjk) {
        // Move the Latin family before the first CJK family
        let latin = families.remove(latin_idx);
        families.insert(cjk_idx, latin);
    }

    families
}

pub(crate) fn style(
    span_id: usize,
    style: &stylo::ComputedValues,
) -> parley::TextStyle<'static, TextBrush> {
    let font_styles = style.get_font();
    let itext_styles = style.get_inherited_text();

    // Convert font size and line height
    let font_size = font_styles.font_size.used_size.0.px();
    let line_height = match font_styles.line_height {
        stylo::LineHeight::Normal => parley::LineHeight::FontSizeRelative(1.2),
        stylo::LineHeight::Number(num) => parley::LineHeight::FontSizeRelative(num.0),
        stylo::LineHeight::Length(value) => parley::LineHeight::Absolute(value.0.px()),
    };

    let letter_spacing = itext_styles
        .letter_spacing
        .0
        .resolve(Length::new(font_size))
        .px();

    // Convert Bold/Italic
    let font_weight = self::font_weight(font_styles.font_weight);
    let font_style = self::font_style(font_styles.font_style);
    let font_width = self::font_width(font_styles.font_stretch);
    let font_variations = self::font_variations(&font_styles.font_variation_settings);

    // Convert font family
    let families: Vec<_> = font_styles
        .font_family
        .families
        .list
        .iter()
        .map(|family| match family {
            stylo::SingleFontFamily::FamilyName(name) => {
                'ret: {
                    let name = name.name.as_ref();

                    // Legacy web compatibility
                    #[cfg(target_vendor = "apple")]
                    if name == "-apple-system" {
                        break 'ret parley::FontFamily::Generic(parley::GenericFamily::SystemUi);
                    }
                    #[cfg(target_os = "macos")]
                    if name == "BlinkMacSystemFont" {
                        break 'ret parley::FontFamily::Generic(parley::GenericFamily::SystemUi);
                    }

                    break 'ret parley::FontFamily::Named(Cow::Owned(name.to_string()));
                }
            }
            stylo::SingleFontFamily::Generic(generic) => {
                parley::FontFamily::Generic(self::generic_font_family(*generic))
            }
        })
        .collect();

    // Reorder font stack: if a CJK font appears before a Latin font in the
    // stack, move the Latin font before the CJK font. This ensures digits
    // and Latin text use proportional-width Latin fonts instead of
    // full-width CJK glyphs.
    let families = reorder_cjk_latin(families);

    // Wrapping and breaking
    let word_break = match itext_styles.word_break {
        stylo::WordBreak::Normal => parley::WordBreakStrength::Normal,
        stylo::WordBreak::BreakAll => parley::WordBreakStrength::BreakAll,
        stylo::WordBreak::KeepAll => parley::WordBreakStrength::KeepAll,
    };
    let overflow_wrap = match itext_styles.overflow_wrap {
        stylo::OverflowWrap::Normal => parley::OverflowWrap::Normal,
        stylo::OverflowWrap::BreakWord => parley::OverflowWrap::BreakWord,
        stylo::OverflowWrap::Anywhere => parley::OverflowWrap::Anywhere,
    };

    parley::TextStyle {
        // font_stack: parley::FontStack::Single(FontFamily::Generic(GenericFamily::SystemUi)),
        font_stack: parley::FontStack::List(Cow::Owned(families)),
        font_size,
        font_width,
        font_style,
        font_weight,
        font_variations: parley::FontSettings::List(Cow::Owned(font_variations)),
        font_features: parley::FontSettings::List(Cow::Borrowed(&[])),
        locale: Default::default(),
        line_height,
        word_spacing: Default::default(),
        letter_spacing,
        overflow_wrap,
        word_break,

        // Contains NodeId
        brush: TextBrush::from_id(span_id),

        // We avoid sending these styles through Parley because they don't affect layout
        // and handling them separately allows us to update them without rebuilding the Parley layout.
        //
        // Instead of setting them here we pass the NodeId in the `brush` field and use that to read these
        // styles lazily when rendering.
        has_underline: Default::default(),
        underline_offset: Default::default(),
        underline_size: Default::default(),
        underline_brush: Default::default(),
        has_strikethrough: Default::default(),
        strikethrough_offset: Default::default(),
        strikethrough_size: Default::default(),
        strikethrough_brush: Default::default(),
    }
}
