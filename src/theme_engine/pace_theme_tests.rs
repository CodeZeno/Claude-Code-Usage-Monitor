use super::*;

const WINDOWS: &[(&str, &str, f64)] = &[
    ("claude-session", "claude.session", 18_000.0),
    ("claude-weekly", "claude.weekly", 604_800.0),
    ("codex-session", "codex.five_hour", 18_000.0),
    ("codex-weekly", "codex.weekly", 604_800.0),
    ("copilot-pool", "copilot.monthly", 31.0 * 86_400.0),
];

fn pace_bundle() -> &'static BundledEditableTheme {
    BUNDLED_EDITABLE_THEMES
        .iter()
        .find(|bundle| bundle.id == COMPACT_FLUENT_PACE_THEME_ID)
        .unwrap()
}

fn pace_theme() -> ThemeDocument {
    let mut theme: ThemeDocument = serde_json::from_str(pace_bundle().source).unwrap();
    theme.prepare_runtime();
    theme
}

fn pace_context(
    binding: &str,
    used: f64,
    remaining: f64,
    dark: bool,
    countdown: bool,
) -> DataContext {
    let runtime = ThemeRuntime::default()
        .with_poll_state(true, false)
        .with_countdown(countdown);
    let mut context = DataContext::from_usage_with_runtime(None, &Canvas::default(), runtime);
    let provider = binding.split('.').next().unwrap();
    context.insert(&format!("{provider}.available"), 1.0);
    context.insert(&format!("{binding}.available"), 1.0);
    context.insert(&format!("{binding}.percentage"), used);
    context.insert(
        &format!("{binding}.display"),
        if countdown { 100.0 - used } else { used },
    );
    context.insert(&format!("{binding}.reset.seconds"), remaining);
    context.insert("system.dark", u8::from(dark).into());
    // Halfway through January, independent of the machine's clock and timezone.
    for (field, value) in [
        ("year", 2026.0),
        ("month", 1.0),
        ("day", 16.0),
        ("hour", 12.0),
        ("minute", 0.0),
    ] {
        context.insert(&format!("time.utc.{field}"), value);
    }
    context
}

fn visible_pace<'a>(theme: &'a ThemeDocument, row: &str, context: &DataContext) -> Vec<&'a str> {
    theme.surfaces[0]
        .children
        .iter()
        .filter(|object| object.id.starts_with(&format!("{row}-pace-")))
        .filter(|object| evaluate(&object.render.0, context).unwrap() != 0.0)
        .map(|object| object.id.as_str())
        .collect()
}

#[test]
fn pace_theme_is_valid_editable_and_bundles_its_assets() {
    let theme = pace_theme();
    assert!(!theme.is_builtin());
    assert!(theme.validate().is_empty(), "{:?}", theme.validate());
    assert_eq!(
        theme.current_time_refresh_interval(),
        Some(Duration::from_secs(60))
    );
    let quad: ThemeDocument =
        serde_json::from_str(include_str!("../themes/compact-fluent-quad.json")).unwrap();
    // Keep the existing tray icons and menus intact.
    assert_eq!(
        serde_json::to_value(&theme.surfaces[1..]).unwrap(),
        serde_json::to_value(&quad.surfaces[1..]).unwrap()
    );
    for (name, bytes) in pace_bundle().assets {
        if name.ends_with(".png") {
            let image = image::load_from_memory(bytes).unwrap();
            assert_eq!((image.width(), image.height()), (56, 56));
            assert!(theme_asset_usage(&theme, &format!("assets/{name}")) > 0);
        } else {
            assert_eq!(*name, "pace-lucide-LICENSE.txt");
            assert!(std::str::from_utf8(bytes).unwrap().contains("ISC License"));
        }
    }
    for object in &theme.surfaces[0].children {
        if let LayerBackground::Image { path, .. } = &object.background {
            assert!(
                pace_bundle()
                    .assets
                    .iter()
                    .any(|(name, _)| path == &format!("assets/{name}")),
                "{path}"
            );
        }
    }
}

#[test]
fn pace_thresholds_include_five_point_boundaries_in_both_display_directions() {
    let theme = pace_theme();
    for &(row, binding, duration) in WINDOWS {
        for dark in [false, true] {
            for countdown in [false, true] {
                for (used, expected) in [
                    (0.0, "under"),
                    (44.99, "under"),
                    (45.0, "on"),
                    (50.0, "on"),
                    (55.0, "on"),
                    (55.01, "over"),
                    (100.0, "over"),
                ] {
                    let context = pace_context(binding, used, duration / 2.0, dark, countdown);
                    let mode = if dark { "dark" } else { "light" };
                    assert_eq!(
                        visible_pace(&theme, row, &context),
                        [format!("{row}-pace-{expected}-{mode}")],
                        "{binding}, used={used}, countdown={countdown}"
                    );
                }
            }
        }
    }
}

#[test]
fn pace_hides_unreliable_or_replaced_windows() {
    let theme = pace_theme();
    for &(row, binding, duration) in WINDOWS {
        let provider = binding.split('.').next().unwrap();
        for (field, value) in [
            ("data.poll_ok".into(), 0.0),
            (format!("{provider}.available"), 0.0),
            (format!("{provider}.stale"), 1.0),
            (format!("{binding}.available"), 0.0),
            (format!("{binding}.reset.seconds"), 0.0),
            (format!("{binding}.reset.seconds"), -1.0),
            (format!("{binding}.reset.seconds"), duration + 1.0),
        ] {
            let mut context = pace_context(binding, 80.0, duration / 2.0, true, false);
            context.insert(&field, value);
            assert!(
                visible_pace(&theme, row, &context).is_empty(),
                "{binding}, {field}={value}"
            );
        }
        if binding.ends_with(".weekly") {
            let mut context = pace_context(binding, 80.0, duration / 2.0, true, false);
            context.insert(&format!("{provider}.credits.available"), 1.0);
            assert!(visible_pace(&theme, row, &context).is_empty());
        }
    }
    // Codex can expose a 30-day quota through its weekly compatibility binding.
    let mut context = pace_context("codex.weekly", 80.0, 86_400.0, true, false);
    context.insert_string("codex.weekly.label", "30d");
    assert!(visible_pace(&theme, "codex-weekly", &context).is_empty());
}

#[test]
fn copilot_pace_uses_calendar_months_and_requires_a_month_boundary_reset() {
    let theme = pace_theme();
    for (year, month, days) in [
        (2026, 2, 28),
        (2028, 2, 29),
        (2000, 2, 29),
        (2100, 2, 28),
        (2026, 4, 30),
        (2026, 12, 31),
    ] {
        let mut context = pace_context(
            "copilot.monthly",
            50.0,
            f64::from(days) * 43_200.0,
            true,
            false,
        );
        context.insert("time.utc.year", year.into());
        context.insert("time.utc.month", month.into());
        context.insert("time.utc.day", (days / 2 + 1).into());
        context.insert("time.utc.hour", ((days % 2) * 12).into());
        assert_eq!(
            visible_pace(&theme, "copilot-pool", &context),
            ["copilot-pool-pace-on-dark"],
            "{year}-{month}"
        );
        // Sub-minute clock differences are expected because resets are in seconds.
        context.insert(
            "copilot.monthly.reset.seconds",
            f64::from(days) * 43_200.0 - 59.0,
        );
        assert_eq!(visible_pace(&theme, "copilot-pool", &context).len(), 1);
        context.insert(
            "copilot.monthly.reset.seconds",
            f64::from(days) * 43_200.0 - 86_400.0,
        );
        assert!(visible_pace(&theme, "copilot-pool", &context).is_empty());
    }
}

#[test]
fn codex_weekly_fallback_does_not_create_a_five_hour_pace() {
    use crate::models::{UsageData, UsageSection};
    let usage = AppUsageData::from_iter([(
        ProviderId::Codex,
        UsageData {
            weekly: UsageSection {
                available: true,
                percentage: 50.0,
                resets_at: Some(std::time::SystemTime::now() + Duration::from_secs(3_600)),
            },
            ..Default::default()
        },
    )]);
    let context = DataContext::from_usage_with_runtime(
        Some(&usage),
        &Canvas::default(),
        ThemeRuntime::default().with_poll_state(true, false),
    );
    assert_eq!(context.get("codex.session.available"), Some(1.0));
    assert_eq!(context.get("codex.five_hour.available"), Some(0.0));
    assert!(visible_pace(&pace_theme(), "codex-session", &context).is_empty());
}

#[test]
fn pace_install_is_independent_of_minecraft_and_preserves_customizations() {
    let themes = themes_directory();
    let assets = assets_directory();
    std::fs::create_dir_all(&assets).unwrap();
    // An existing user has installed (and possibly deleted) Minecraft already.
    std::fs::write(themes.join(".minecraft-theme-installed"), b"1").unwrap();
    let path = themes.join(format!("{COMPACT_FLUENT_PACE_THEME_ID}.json"));
    let mut custom = pace_theme();
    custom.name = "My pace theme".into();
    crate::app_settings::write_json_atomic(&path, &custom).unwrap();
    let asset = assets.join("pace-lucide-codex-on-dark.png");
    std::fs::write(&asset, b"custom image bytes").unwrap();

    ensure_bundled_editable_themes(&themes, &assets).unwrap();
    assert_eq!(load_theme(&path).unwrap().name, "My pace theme");
    assert_eq!(std::fs::read(&asset).unwrap(), b"custom image bytes");
    assert!(themes.join(pace_bundle().install_marker).is_file());
    assert!(!themes.join(format!("{MINECRAFT_THEME_ID}.json")).exists());
    for (name, _) in pace_bundle().assets {
        assert!(assets.join(name).is_file(), "{name}");
    }

    std::fs::remove_file(&path).unwrap();
    ensure_bundled_editable_themes(&themes, &assets).unwrap();
    assert!(
        !path.exists(),
        "deliberately deleted themes must stay deleted"
    );
}

#[test]
fn pace_layout_reserves_icon_space_only_for_supported_providers() {
    let theme = pace_theme();
    let mut quad: ThemeDocument =
        serde_json::from_str(include_str!("../themes/compact-fluent-quad.json")).unwrap();
    quad.prepare_runtime();
    for providers in [
        ProviderSet::from_enabled([ProviderId::Claude]),
        ProviderSet::from_enabled([ProviderId::Codex, ProviderId::Copilot]),
        ProviderSet::from_enabled([ProviderId::Cursor, ProviderId::Grok]),
        ProviderSet::from_enabled(ProviderId::ALL),
    ] {
        let runtime = ThemeRuntime::from_providers(providers);
        let supported = providers
            .iter()
            .filter(|id| {
                matches!(
                    id,
                    ProviderId::Claude | ProviderId::Codex | ProviderId::Copilot
                )
            })
            .count() as u32;
        let (base_width, base_height) = resolve_surface_size(&quad, 0, None, runtime);
        assert_eq!(
            resolve_surface_size(&theme, 0, None, runtime),
            (base_width + 20 * supported, base_height)
        );
    }
}

#[test]
fn pace_renders_dark_and_light_at_multiple_scales() {
    use crate::models::{UsageData, UsageSection};
    ensure_bundled_editable_themes(&themes_directory(), &assets_directory()).unwrap();
    let now = std::time::SystemTime::now();
    let window = |percentage, seconds| UsageSection {
        available: true,
        percentage,
        resets_at: Some(now + Duration::from_secs(seconds)),
    };
    let usage = AppUsageData::from_iter([
        (
            ProviderId::Claude,
            UsageData {
                session: window(20.0, 9_000),
                weekly: window(50.0, 302_400),
                ..Default::default()
            },
        ),
        (
            ProviderId::Codex,
            UsageData {
                session: window(80.0, 9_000),
                weekly: window(20.0, 302_400),
                ..Default::default()
            },
        ),
        (
            ProviderId::Copilot,
            UsageData {
                weekly: window(50.0, 31 * 43_200),
                weekly_label: Some("30d".into()),
                monthly: Some(window(50.0, 31 * 43_200)),
                ..Default::default()
            },
        ),
    ]);
    let runtime = ThemeRuntime::from_providers(ProviderSet::from_enabled([
        ProviderId::Claude,
        ProviderId::Codex,
        ProviderId::Copilot,
    ]))
    .with_poll_state(true, false);
    for (mode, dark, background) in [("dark", "1", "#202020"), ("light", "0", "#F3F3F3")] {
        // Fix the preview clock and colour mode without changing OS settings.
        let source = pace_bundle()
            .source
            .replace("system.dark", dark)
            .replace("time.utc.year", "2026")
            .replace("time.utc.month", "1")
            .replace("time.utc.day", "16")
            .replace("time.utc.hour", "12")
            .replace("time.utc.minute", "0");
        let mut theme: ThemeDocument = serde_json::from_str(&source).unwrap();
        theme.surfaces[0].background = LayerBackground::Colour {
            colour: Paint::new(background),
        };
        theme.prepare_runtime();
        let context = DataContext::from_usage_with_runtime(Some(&usage), &theme.canvas, runtime);
        for (row, expected) in [
            ("claude-session", "under"),
            ("claude-weekly", "on"),
            ("codex-session", "over"),
            ("codex-weekly", "under"),
            ("copilot-pool", "on"),
        ] {
            assert_eq!(
                visible_pace(&theme, row, &context),
                [format!("{row}-pace-{expected}-{mode}")]
            );
        }
        let mut without_icons = theme.clone();
        for object in &mut without_icons.surfaces[0].children {
            if object.id.contains("-pace-") {
                object.render = 0.0.into();
            }
        }
        for scale in [1.0, 1.25, 2.0] {
            let rendered =
                render_theme_surface_with_runtime_at_scale(&theme, 0, Some(&usage), runtime, scale);
            assert!(
                rendered.warnings.is_empty(),
                "{mode}/{scale}: {:?}",
                rendered.warnings
            );
            let plain = render_theme_surface_with_runtime_at_scale(
                &without_icons,
                0,
                Some(&usage),
                runtime,
                scale,
            );
            assert_ne!(
                rendered.pixels, plain.pixels,
                "pace icons must draw visible pixels"
            );

            // Optional native-rendered documentation fixtures, using only dummy usage.
            if let Some(directory) =
                std::env::var_os("CCUM_PACE_PREVIEW_DIR").filter(|_| scale == 2.0)
            {
                let directory = PathBuf::from(directory);
                std::fs::create_dir_all(&directory).unwrap();
                let image = image::RgbaImage::from_fn(rendered.width, rendered.height, |x, y| {
                    let pixel = rendered.pixels[(y * rendered.width + x) as usize];
                    let alpha = pixel >> 24;
                    let channel = |shift: u32| {
                        if alpha == 0 {
                            0
                        } else {
                            (((pixel >> shift) & 255) * 255 / alpha).min(255) as u8
                        }
                    };
                    image::Rgba([channel(16), channel(8), channel(0), alpha as u8])
                });
                image
                    .save(directory.join(format!("compact-fluent-pace-{mode}.png")))
                    .unwrap();
            }
        }
    }
}
