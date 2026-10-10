use super::*;
use crate::models::{AccountUsage, UsageData, UsageLimit, UsageSection};
use std::time::SystemTime;

const HOUR: u64 = 3_600;
const DAY: u64 = 86_400;
/// Bar height: 80 of content over a 4 px margin for the card's 1 px drop.
const BAR_HEIGHT: u32 = 84;
/// Margin plus card padding either side of the providers.
const EDGE: f64 = 20.0;

fn top_bar() -> ThemeDocument {
    let (_, source) = BUILTIN_THEME_SOURCES
        .iter()
        .find(|(id, _)| *id == TOP_BAR_THEME_ID)
        .expect("Top Bar is a built-in theme");
    let mut theme: ThemeDocument = serde_json::from_str(source).unwrap();
    theme.prepare_runtime();
    theme
}

fn section(percentage: f64, resets_in: Option<u64>) -> UsageSection {
    UsageSection {
        available: true,
        percentage,
        // A little slack keeps whole-minute countdowns stable while a test runs.
        resets_at: resets_in.map(|seconds| {
            SystemTime::now() + Duration::from_secs(seconds) + Duration::from_secs(30)
        }),
    }
}

fn fixture(fable: bool) -> AppUsageData {
    let mut claude = UsageData {
        session: section(21.0, Some(3 * HOUR + 27 * 60)),
        weekly: section(4.0, Some(3 * DAY + 7 * HOUR)),
        ..Default::default()
    };
    if fable {
        claude.limits.push(UsageLimit {
            key: "weekly_scoped_fable".into(),
            kind: "weekly_scoped".into(),
            label: "Fable".into(),
            model: Some("Fable".into()),
            usage: section(0.0, Some(3 * DAY + 7 * HOUR)),
            ..Default::default()
        });
    }
    let codex = UsageData {
        session: UsageSection::default(),
        weekly: section(87.0, Some(4 * DAY + HOUR)),
        ..Default::default()
    };
    let cursor = UsageData {
        session: section(41.0, Some(2 * HOUR + 9 * 60)),
        weekly: section(23.0, Some(3 * DAY + 4 * HOUR)),
        weekly_label: Some("API".into()),
        ..Default::default()
    };
    AppUsageData::from_iter([
        (ProviderId::Claude, claude),
        (ProviderId::Codex, codex),
        (ProviderId::Cursor, cursor),
    ])
}

fn runtime(providers: &[ProviderId], host_width: u32) -> ThemeRuntime {
    let updated = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - 20;
    ThemeRuntime::from_providers(ProviderSet::from_enabled(providers.iter().copied()))
        .with_nest(SurfaceNest::Floating)
        .with_host_dimensions(host_width, 1080)
        .with_updated_unix(Some(updated))
        .with_system_dark(false)
}

fn alarm(enabled: bool, active: bool) -> ThemeAlarm {
    ThemeAlarm {
        enabled,
        active,
        snoozed: false,
    }
}

/// A layer's paint: its background colour, or its text colour.
fn layer_colour(
    theme: &ThemeDocument,
    id: &str,
    data: &AppUsageData,
    runtime: ThemeRuntime,
) -> Rgba {
    let layer = &theme.surfaces[0].children[layer_index(theme, id)];
    let paint = match (&layer.background, &layer.content) {
        (LayerBackground::Colour { colour }, _) => colour,
        (_, SceneContent::Text { color, .. }) => color,
        _ => panic!("{id} has no flat colour"),
    };
    paint.resolve(&context(theme, Some(data), runtime))
}

fn rgb(hex: &str) -> Rgba {
    parse_color(hex).unwrap()
}

fn codex_weekly_at(percentage: f64) -> AppUsageData {
    let mut data = fixture(true);
    data.insert(
        ProviderId::Codex,
        UsageData {
            weekly: section(percentage, Some(4 * DAY)),
            ..Default::default()
        },
    );
    data
}

fn layer_index(theme: &ThemeDocument, id: &str) -> usize {
    theme.surfaces[0]
        .children
        .iter()
        .position(|layer| layer.id == id)
        .unwrap_or_else(|| panic!("missing layer {id}"))
}

fn bounds(
    theme: &ThemeDocument,
    id: &str,
    data: &AppUsageData,
    runtime: ThemeRuntime,
) -> Option<(f64, f64, f64, f64)> {
    resolve_object_bounds_with_runtime(theme, 0, layer_index(theme, id), Some(data), runtime)
}

fn context(
    theme: &ThemeDocument,
    data: Option<&AppUsageData>,
    runtime: ThemeRuntime,
) -> DataContext {
    let (width, height) = resolve_surface_content_size(theme, 0, data, runtime);
    DataContext::from_usage_with_runtime(
        data,
        &Canvas {
            width,
            height,
            ..Canvas::default()
        },
        runtime,
    )
}

#[test]
fn countdown_format_counts_minutes_hours_and_days() {
    let context = DataContext::default();
    for (seconds, expected) in [
        (-5.0, "<1m"),
        (0.0, "<1m"),
        (59.9, "<1m"),
        (60.0, "1m"),
        (14.0 * 60.0 + 59.0, "14m"),
        (3_599.0, "59m"),
        (3_600.0, "1h 0m"),
        (2.0 * 3_600.0 + 14.0 * 60.0 + 59.0, "2h 14m"),
        (86_399.0, "23h 59m"),
        (86_400.0, "1d 0h"),
        (3.0 * 86_400.0 + 4.0 * 3_600.0 + 3_599.0, "3d 4h"),
    ] {
        let mut context = context.clone();
        context.insert("left", seconds);
        assert_eq!(
            format_template("{left:countdown}", &context),
            expected,
            "{seconds}"
        );
    }
}

#[test]
fn countdown_and_update_age_refresh_the_surface_every_minute() {
    let mut theme = ThemeDocument::starter();
    assert_eq!(theme.current_time_refresh_interval(), None);
    let mut layer = SceneObject::object("left", "Time left");
    layer.content = SceneContent::Text {
        template: "{claude.session.reset.seconds:countdown}".into(),
        font_family: "Segoe UI".into(),
        font_size: 12.0.into(),
        weight: FontWeight::Regular,
        rendering: FontRendering::Antialiased,
        contrast: 1.0.into(),
        align: TextAlign::Left,
        color: Paint::new("#FFFFFFFF"),
    };
    theme.surfaces[0].children.push(layer);
    assert_eq!(
        theme.current_time_refresh_interval(),
        Some(Duration::from_secs(60))
    );
    assert_eq!(
        top_bar().current_time_refresh_interval(),
        Some(Duration::from_secs(60))
    );
}

#[test]
fn update_age_is_exposed_only_when_known() {
    let unknown = DataContext::from_usage(None, &Canvas::default());
    assert_eq!(unknown.get("data.updated.available"), Some(0.0));
    assert_eq!(unknown.get("data.updated.seconds"), Some(0.0));
    let known = DataContext::from_usage_with_runtime(
        None,
        &Canvas::default(),
        runtime(&[ProviderId::Claude], 1920),
    );
    assert_eq!(known.get("data.updated.available"), Some(1.0));
    let age = known.get("data.updated.seconds").unwrap();
    assert!((19.0..30.0).contains(&age), "{age}");
    assert_eq!(
        format_template("{data.updated.seconds:countdown}", &known),
        "<1m"
    );
    assert_eq!(
        ThemeRuntime::default().with_updated_unix(Some(0)),
        ThemeRuntime::default()
    );
}

#[test]
fn top_bar_is_a_top_centre_auto_hiding_floating_bar() {
    let theme = top_bar();
    assert!(theme.is_builtin());
    assert_eq!(theme.name, "Top Bar");
    assert!(theme.validate().is_empty(), "{:?}", theme.validate());
    let placement = &theme.surfaces[0].placement;
    assert_eq!(placement.reference.region, ReferenceRegion::Monitor);
    assert_eq!(placement.nest, SurfaceNest::Floating);
    assert_eq!(
        (placement.horizontal, placement.surface_horizontal),
        (HorizontalAnchor::Center, Some(HorizontalAnchor::Center))
    );
    assert_eq!(placement.auto_hide_edge(), Some(SurfaceEdge::Top));
    assert_eq!(
        theme.edge_surface().map(|surface| surface.id.as_str()),
        Some("main")
    );
    // The bar paints its own card: no automatic floating card or inset.
    assert_eq!(
        surface_horizontal_padding(&theme, 0, runtime(&[ProviderId::Claude], 1920)),
        0
    );
}

#[test]
fn top_bar_lays_providers_out_left_to_right_within_the_monitor() {
    let theme = top_bar();
    let data = fixture(true);
    for host_width in [1920, 1280] {
        let mut previous_width = 0;
        // Widths of the first approved Top Bar (92 px tall); the restyle may
        // not grow it.
        for (providers, approved) in [
            (vec![ProviderId::Claude], 424),
            (vec![ProviderId::Claude, ProviderId::Codex], 705),
            (
                vec![ProviderId::Claude, ProviderId::Codex, ProviderId::Cursor],
                986,
            ),
        ] {
            let runtime = runtime(&providers, host_width);
            let (width, height) = resolve_surface_size(&theme, 0, Some(&data), runtime);
            assert_eq!(height, BAR_HEIGHT);
            assert!(width <= approved, "{providers:?}: {width} > {approved}");
            assert!(
                width > previous_width,
                "{providers:?} at {host_width}: {width}"
            );
            assert!(
                width <= host_width - 16,
                "{providers:?}: {width} > {host_width}"
            );
            previous_width = width;
            let mut right = 0.0;
            for provider in &providers {
                let id = format!("{}-block", provider.descriptor().key);
                let (x, _, w, _) = bounds(&theme, &id, &data, runtime).unwrap();
                assert!(x >= right, "{id} starts at {x}, before {right}");
                assert!(x + w <= width as f64 - EDGE + 0.5, "{id} ends at {}", x + w);
                right = x + w;
            }
            for hidden in ProviderId::ALL.iter().filter(|p| !providers.contains(p)) {
                let id = format!("{}-block", hidden.descriptor().key);
                assert!(bounds(&theme, &id, &data, runtime).is_none(), "{id}");
            }
            let rendered = render_theme_surface_with_runtime(&theme, 0, Some(&data), runtime);
            assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
            assert_eq!((rendered.width, rendered.height), (width, height));
        }
    }
}

#[test]
fn top_bar_shrinks_every_provider_into_a_narrow_monitor() {
    let theme = top_bar();
    let mut data = fixture(true);
    for provider in [
        ProviderId::Antigravity,
        ProviderId::OpenCode,
        ProviderId::Grok,
        ProviderId::Copilot,
    ] {
        data.insert(
            provider,
            UsageData {
                session: section(10.0, Some(HOUR)),
                weekly: section(20.0, Some(DAY)),
                ..Default::default()
            },
        );
    }
    let runtime = runtime(&ProviderId::ALL, 1280);
    let (width, _) = resolve_surface_size(&theme, 0, Some(&data), runtime);
    assert_eq!(width, 1280 - 16);
    let (x, _, w, _) = bounds(&theme, "opencode-block", &data, runtime).unwrap();
    assert!(
        x + w <= width as f64 - EDGE + 1.0,
        "last CLI ends at {}",
        x + w
    );
    let (_, _, cell, _) = bounds(&theme, "claude-five_hour", &data, runtime).unwrap();
    assert!(cell < 112.0 && cell > 40.0, "{cell}");
}

#[test]
fn claude_shows_the_fable_weekly_cap_only_when_reported() {
    let theme = top_bar();
    let runtime = runtime(&[ProviderId::Claude, ProviderId::Codex], 1920);
    let with = fixture(true);
    let without = fixture(false);
    assert!(bounds(&theme, "claude-fable", &with, runtime).is_some());
    assert!(bounds(&theme, "claude-fable", &without, runtime).is_none());
    let wide = bounds(&theme, "claude-block", &with, runtime).unwrap().2;
    let narrow = bounds(&theme, "claude-block", &without, runtime).unwrap().2;
    assert_eq!(wide - narrow, 128.0);
    // Codex has no five-hour window here: its cell stays, marked n/a.
    assert!(bounds(&theme, "codex-five_hour-na", &with, runtime).is_some());
    assert!(bounds(&theme, "codex-five_hour-value", &with, runtime).is_none());
    assert!(bounds(&theme, "codex-weekly-value", &with, runtime).is_some());
    assert!(bounds(&theme, "codex-weekly-pill", &with, runtime).is_some());
    assert!(bounds(&theme, "claude-weekly-pill", &with, runtime).is_none());
}

#[test]
fn top_bar_status_and_handle_follow_freshness_and_usage() {
    let theme = top_bar();
    let providers = [ProviderId::Claude, ProviderId::Codex];
    let mut data = fixture(true);
    let live = runtime(&providers, 1920);
    let handle = |data: &AppUsageData, runtime| {
        let expression = theme.surfaces[0].placement.handle_color.as_ref().unwrap();
        evaluate_color(&expression.0, &context(&theme, Some(data), runtime)).unwrap()
    };
    let status =
        |id: &str, data: &AppUsageData, runtime| bounds(&theme, id, data, runtime).is_some();
    assert!(status("status-live", &data, live));
    assert!(status("status-live-pill", &data, live));
    assert!(!status("status-stale", &data, live));
    let SceneContent::Text { template, .. } =
        &theme.surfaces[0].children[layer_index(&theme, "status-live")].content
    else {
        panic!("status-live is a text layer");
    };
    assert_eq!(
        format_template(template, &context(&theme, Some(&data), live)),
        "Live \u{b7} updated <1m ago"
    );
    // Live is a green "working" pill: tinted fill, dark green text, green dot.
    for (dark, tint, ink, dot) in [
        (false, "#E4FCEF", "#0F7A4A", "#18C669"),
        (true, "#2FD27F1F", "#4ADE94", "#2FD27F"),
    ] {
        let mode = live.with_system_dark(dark);
        assert_eq!(
            layer_colour(&theme, "status-live-pill", &data, mode),
            rgb(tint)
        );
        assert_eq!(layer_colour(&theme, "status-live", &data, mode), rgb(ink));
        assert_eq!(
            layer_colour(&theme, "status-live-dot", &data, mode),
            rgb(dot)
        );
    }
    // Codex weekly is at 87%: the collapsed handle warns in amber.
    assert_eq!(handle(&data, live), rgb("#FFA100"));
    assert_eq!(handle(&data, live.with_system_dark(true)), rgb("#FFB020"));
    data.insert(
        ProviderId::Claude,
        UsageData {
            session: section(93.0, Some(HOUR)),
            stale: true,
            ..Default::default()
        },
    );
    assert!(status("status-stale", &data, live));
    assert!(!status("status-live", &data, live));
    let SceneContent::Text { template, .. } =
        &theme.surfaces[0].children[layer_index(&theme, "status-stale")].content
    else {
        panic!("status-stale is a text layer");
    };
    assert_eq!(
        format_template(template, &context(&theme, Some(&data), live)),
        "Stale \u{b7} updated <1m ago"
    );
    // Stale is an amber "waiting" pill, and the handle goes quiet.
    assert_eq!(
        layer_colour(&theme, "status-stale-pill", &data, live),
        rgb("#FFF4E0")
    );
    assert_eq!(
        layer_colour(&theme, "status-stale", &data, live),
        rgb("#8A5600")
    );
    assert_eq!(handle(&data, live), rgb("#C7C7C7"));
    let mut fresh = data.clone();
    fresh.insert(
        ProviderId::Claude,
        UsageData {
            session: section(93.0, Some(HOUR)),
            ..Default::default()
        },
    );
    assert_eq!(handle(&fresh, live), rgb("#EF4444"));
    let calm = AppUsageData::from_iter([(
        ProviderId::Claude,
        UsageData {
            session: section(10.0, Some(HOUR)),
            ..Default::default()
        },
    )]);
    assert_eq!(handle(&calm, live), rgb("#6B6B6B"));
    assert_eq!(handle(&calm, live.with_system_dark(true)), rgb("#9B9BA2"));
    let loading = live.with_poll_state(false, false);
    assert!(status("status-waiting", &calm, loading));
    assert!(status("status-waiting-pill", &calm, loading));
}

#[test]
fn the_longest_status_pill_clears_the_last_providers_name() {
    let theme = top_bar();
    let mut data = fixture(true);
    data.insert(
        ProviderId::Claude,
        UsageData {
            stale: true,
            ..data.get(ProviderId::Claude).unwrap().clone()
        },
    );
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    // GDI widths of the last provider's 12 px semibold name.
    for (providers, name_width) in [
        (vec![ProviderId::Claude], 42.0),
        (vec![ProviderId::Claude, ProviderId::Codex], 38.0),
        (
            vec![ProviderId::Claude, ProviderId::Codex, ProviderId::Cursor],
            40.0,
        ),
    ] {
        let runtime =
            runtime(&providers, 1920).with_updated_unix(Some(now - (23 * HOUR + 59 * 60 + 30)));
        let SceneContent::Text { template, .. } =
            &theme.surfaces[0].children[layer_index(&theme, "status-stale")].content
        else {
            panic!("status-stale is a text layer");
        };
        assert_eq!(
            format_template(template, &context(&theme, Some(&data), runtime)),
            "Stale \u{b7} updated 23h 59m ago"
        );
        let (width, _) = resolve_surface_size(&theme, 0, Some(&data), runtime);
        let last = providers.last().unwrap().descriptor().key;
        let (block, ..) = bounds(&theme, &format!("{last}-block"), &data, runtime).unwrap();
        let (pill, _, pill_width, _) = bounds(&theme, "status-stale-pill", &data, runtime).unwrap();
        assert!(
            pill >= block + 17.0 + name_width + 16.0 - 0.5,
            "{last}: pill at {pill}"
        );
        assert!(pill + pill_width <= width as f64 - EDGE + 0.5);
    }
}

#[test]
fn handle_is_a_flat_edged_tab_matching_the_card_and_tinted_by_its_expression() {
    let theme = top_bar();
    let data = fixture(true);
    for (dark, card) in [(false, 0xFF), (true, 0x19)] {
        let runtime =
            runtime(&[ProviderId::Claude, ProviderId::Codex], 1920).with_system_dark(dark);
        for (scale, size) in [(1.0, (64, 6)), (1.5, (96, 9))] {
            for edge in [SurfaceEdge::Top, SurfaceEdge::Bottom] {
                let handle = render_surface_handle(&theme, 0, Some(&data), runtime, scale, edge);
                assert!(handle.warnings.is_empty(), "{:?}", handle.warnings);
                assert_eq!((handle.width, handle.height), size);
                let pixel = |x: u32, y: u32| handle.pixels[(y * handle.width + x) as usize];
                let (flat, round) = match edge {
                    SurfaceEdge::Top => (0, handle.height - 1),
                    SurfaceEdge::Bottom => (handle.height - 1, 0),
                };
                // Corners touching the screen edge are square; the far ones rounded.
                assert!(pixel(0, flat) >> 24 > 200, "{edge:?} {scale}");
                assert!(pixel(0, round) >> 24 < 200, "{edge:?} {scale}");
                // The tab is the card's own surface colour in either mode.
                let fill = pixel(handle.width / 4, handle.height / 2);
                assert!(
                    (i32::from((fill >> 8 & 0xff) as u8) - card).abs() <= 6,
                    "dark={dark} {fill:08x}"
                );
                // The amber accent line sits in the middle of the tab.
                let middle = pixel(handle.width / 2, handle.height / 2);
                assert!((middle >> 16 & 0xff) > (middle & 0xff) + 40, "{middle:08x}");
            }
        }
    }
}

#[test]
fn edge_preferences_override_hiding_and_display_only_for_edge_surfaces() {
    let mut theme = top_bar();
    theme.apply_edge_preferences(Some(false), Some(1));
    assert!(!theme.surfaces[0].placement.auto_hide);
    assert_eq!(theme.surfaces[0].placement.reference.display, 1);
    assert_eq!(theme.placement, theme.surfaces[0].placement);
    assert_eq!(theme.surfaces[0].placement.edge(), Some(SurfaceEdge::Top));
    assert_eq!(theme.surfaces[0].placement.auto_hide_edge(), None);

    let mut classic = ThemeDocument::starter();
    let before = serde_json::to_string(&classic).unwrap();
    classic.apply_edge_preferences(Some(true), Some(1));
    assert_eq!(serde_json::to_string(&classic).unwrap(), before);
    assert!(classic.edge_surface().is_none());
}

#[test]
fn auto_hide_needs_a_floating_root_docked_inside_a_top_or_bottom_edge() {
    let mut placement = top_bar().surfaces[0].placement.clone();
    assert_eq!(placement.auto_hide_edge(), Some(SurfaceEdge::Top));
    placement.vertical = VerticalAnchor::Bottom;
    placement.surface_vertical = Some(VerticalAnchor::Bottom);
    assert_eq!(placement.auto_hide_edge(), Some(SurfaceEdge::Bottom));
    placement.surface_vertical = Some(VerticalAnchor::Top);
    assert_eq!(placement.edge(), None);
    placement.vertical = VerticalAnchor::Center;
    placement.surface_vertical = Some(VerticalAnchor::Center);
    assert_eq!(placement.edge(), None);
    placement.vertical = VerticalAnchor::Top;
    placement.surface_vertical = None;
    placement.nest = SurfaceNest::Taskbar;
    assert_eq!(placement.edge(), None);
    placement.nest = SurfaceNest::Floating;
    placement.auto_hide = false;
    assert_eq!(
        (placement.edge(), placement.auto_hide_edge()),
        (Some(SurfaceEdge::Top), None)
    );
    // Themes that never set it keep their saved form byte for byte.
    let json = serde_json::to_value(ThemeDocument::starter()).unwrap();
    for field in [
        "auto_hide",
        "handle_color",
        "handle_background",
        "force_reveal",
    ] {
        assert!(!json.to_string().contains(field), "{field}");
    }
}

#[test]
fn handle_colour_must_be_colour_text() {
    let mut theme = top_bar();
    theme.surfaces[0].placement.handle_color = Some(Expression("42".into()));
    assert!(theme
        .validate()
        .iter()
        .any(|error| error.contains("handle_color")));
    theme.surfaces[0].placement.handle_color = Some(Expression("\"#12345\"".into()));
    assert!(theme
        .validate()
        .iter()
        .any(|error| error.contains("handle_color")));
    theme.surfaces[0].placement.handle_color = Some(Expression(
        "if(claude.session.percentage > 50, \"#FF0000\", \"#00FF00AA\")".into(),
    ));
    assert!(theme.validate().is_empty(), "{:?}", theme.validate());
    theme.surfaces[0].placement.handle_background = Some(Expression("1".into()));
    assert!(theme
        .validate()
        .iter()
        .any(|error| error.contains("handle_background")));
    theme.surfaces[0].placement.handle_background = None;
    theme.surfaces[0].placement.force_reveal = Some(Expression("alarm.actve".into()));
    assert!(theme
        .validate()
        .iter()
        .any(|error| error.contains("force_reveal")));
    // Layer colours accept colour expressions too, and reject bad ones.
    let card = layer_index(&theme, "card");
    theme.surfaces[0].placement.force_reveal = None;
    theme.surfaces[0].children[card].background = LayerBackground::Colour {
        colour: Paint::new("if(system.dark, 7, 8)"),
    };
    assert!(theme.validate().iter().any(|error| error.contains("Card")));
}

/// Every colour in the theme, evaluated in one mode: background, border and
/// text paints of every layer plus the handle's two placement colours.
fn every_colour(
    theme: &ThemeDocument,
    context: &DataContext,
) -> Vec<(String, Result<Rgba, String>)> {
    let surface = &theme.surfaces[0];
    let mut colours = Vec::new();
    let mut add = |label: String, colour: &str| {
        let resolved = parse_color(colour)
            .ok_or(String::new())
            .or_else(|_| evaluate_color(colour, context));
        colours.push((label, resolved));
    };
    for layer in std::iter::once(surface).chain(&surface.children) {
        if let LayerBackground::Colour { colour } = &layer.background {
            add(format!("{}.background", layer.id), &colour.color);
        }
        if let Some(border) = &layer.border {
            add(format!("{}.border", layer.id), &border.color.color);
        }
        if let SceneContent::Text { color, .. } = &layer.content {
            add(format!("{}.color", layer.id), &color.color);
        }
    }
    for (name, expression) in [
        ("handle_color", &surface.placement.handle_color),
        ("handle_background", &surface.placement.handle_background),
    ] {
        add(name.into(), &expression.as_ref().unwrap().0);
    }
    colours
}

#[test]
fn light_and_dark_variants_both_validate_and_differ() {
    let theme = top_bar();
    assert!(theme.validate().is_empty(), "{:?}", theme.validate());
    let data = codex_weekly_at(93.0);
    let base = runtime(&[ProviderId::Claude, ProviderId::Codex], 1920);
    let [light, dark] = [false, true].map(|dark| {
        let colours = every_colour(
            &theme,
            &context(&theme, Some(&data), base.with_system_dark(dark)),
        );
        assert!(colours.len() > 150, "{}", colours.len());
        let failures: Vec<_> = colours
            .iter()
            .filter(|(_, colour)| colour.is_err())
            .collect();
        assert!(failures.is_empty(), "dark={dark}: {failures:?}");
        colours
    });
    // The card and its text really switch, not just one layer.
    let pick = |colours: &[(String, Result<Rgba, String>)], id: &str| {
        colours
            .iter()
            .find(|(label, _)| label == id)
            .unwrap_or_else(|| panic!("no {id}"))
            .1
            .clone()
            .unwrap()
    };
    for (id, light_hex, dark_hex) in [
        ("card.background", "#FFFFFF", "#19191C"),
        ("card.border", "#ECECEC", "#FFFFFF14"),
        ("claude-name.color", "#171717", "#EDEDEF"),
        ("claude-weekly-label.color", "#6B6B6B", "#9B9BA2"),
        ("claude-weekly-track.background", "#EBEBEB", "#2A2A2F"),
        ("codex-divider.background", "#EBEBEB", "#2A2A2F"),
        ("handle_background", "#FFFFFF", "#19191C"),
    ] {
        assert_eq!(pick(&light, id), rgb(light_hex), "{id}");
        assert_eq!(pick(&dark, id), rgb(dark_hex), "{id}");
    }
    let changed = light
        .iter()
        .zip(&dark)
        .filter(|((_, light), (_, dark))| light != dark)
        .count();
    assert!(changed > 100, "only {changed} colours follow the mode");
}

#[test]
fn level_colours_switch_at_70_and_90_in_both_modes() {
    let theme = top_bar();
    let providers = [ProviderId::Claude, ProviderId::Codex];
    // (used %, pill shown, value text, bar fill) per mode.
    let light = [
        (69.0, false, "#171717", "#171717"),
        (70.0, true, "#8A5600", "#FFA100"),
        (89.0, true, "#8A5600", "#FFA100"),
        (90.0, true, "#B42318", "#EF4444"),
        (93.0, true, "#B42318", "#EF4444"),
    ];
    let dark = [
        (69.0, false, "#EDEDEF", "#EDEDEF"),
        (70.0, true, "#FFC35C", "#FFB020"),
        (89.0, true, "#FFC35C", "#FFB020"),
        (90.0, true, "#FF8F8F", "#F26B6B"),
        (93.0, true, "#FF8F8F", "#F26B6B"),
    ];
    for (is_dark, levels) in [(false, light), (true, dark)] {
        let runtime = runtime(&providers, 1920).with_system_dark(is_dark);
        for (percentage, pill, value, fill) in levels {
            let data = codex_weekly_at(percentage);
            let label = format!("dark={is_dark} {percentage}%");
            assert_eq!(
                bounds(&theme, "codex-weekly-pill", &data, runtime).is_some(),
                pill,
                "{label}"
            );
            assert_eq!(
                layer_colour(&theme, "codex-weekly-value", &data, runtime),
                rgb(value),
                "{label}"
            );
            assert_eq!(
                layer_colour(&theme, "codex-weekly-fill", &data, runtime),
                rgb(fill),
                "{label}"
            );
        }
        // The pill behind the percentage is the matching tint.
        for (percentage, light_tint, dark_tint) in [
            (75.0, "#FFF4E0", "#FFB0201F"),
            (93.0, "#FEF0EF", "#F26B6B1F"),
        ] {
            assert_eq!(
                layer_colour(
                    &theme,
                    "codex-weekly-pill",
                    &codex_weekly_at(percentage),
                    runtime
                ),
                rgb(if is_dark { dark_tint } else { light_tint })
            );
        }
    }
}

/// Keys of the quotas the alarm watches that are at or below 5% left.
fn low_keys(data: &AppUsageData, enabled: ProviderSet) -> Vec<String> {
    alarm_scan(data, enabled)
        .readings
        .into_iter()
        .filter(|reading| reading.low)
        .map(|reading| reading.key)
        .collect()
}

fn cap(key: &str, kind: &str, model: &str, percentage: f64) -> UsageLimit {
    UsageLimit {
        key: key.into(),
        kind: kind.into(),
        label: model.into(),
        model: Some(model.into()),
        usage: section(percentage, Some(DAY)),
        ..Default::default()
    }
}

#[test]
fn the_alarm_starts_at_five_percent_left_for_enabled_providers_whatever_the_theme_shows() {
    let enabled = ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Codex]);
    // Exactly 5% left alarms; 6% left does not.
    assert_eq!(low_keys(&codex_weekly_at(95.0), enabled), ["codex.weekly"]);
    assert!(low_keys(&codex_weekly_at(94.0), enabled).is_empty());
    // What reads as 5% once rounded counts; what reads as 6% does not.
    assert_eq!(low_keys(&codex_weekly_at(94.6), enabled), ["codex.weekly"]);
    assert!(low_keys(&codex_weekly_at(94.5), enabled).is_empty());
    // A disabled provider never alarms.
    let mut data = codex_weekly_at(10.0);
    data.insert(
        ProviderId::Cursor,
        UsageData {
            session: section(99.0, Some(HOUR)),
            ..Default::default()
        },
    );
    assert!(low_keys(&data, enabled).is_empty());
    // Model caps count (Fable and Sonnet alike), whatever the active theme binds.
    let mut claude = data.get(ProviderId::Claude).unwrap().clone();
    for (key, kind, model) in [
        ("weekly_scoped_fable", "weekly_scoped", "Fable"),
        ("seven_day_sonnet", "seven_day_sonnet", "Sonnet"),
        ("seven_day_opus", "seven_day_opus", "Opus"),
    ] {
        claude.limits.retain(|limit| limit.key != key);
        claude.limits.push(cap(key, kind, model, 97.0));
    }
    data.insert(ProviderId::Claude, claude);
    assert_eq!(
        low_keys(&data, enabled),
        [
            "claude.model.fable",
            "claude.model.sonnet",
            "claude.model.opus"
        ]
    );
}

#[test]
fn limits_that_duplicate_a_window_or_have_an_unknown_kind_never_count() {
    let enabled = ProviderSet::from_enabled([ProviderId::Claude]);
    let mut usage = UsageData {
        session: section(29.0, Some(HOUR)),
        weekly: section(26.0, Some(DAY)),
        ..Default::default()
    };
    // The shape of a real usage cache: session and weekly_all repeat the
    // windows, iguana_necktie is opaque.
    for (key, kind, percentage) in [
        ("session", "session", 99.0),
        ("weekly_all", "weekly_all", 99.0),
        ("iguana_necktie", "iguana_necktie", 99.0),
    ] {
        usage.limits.push(cap(key, kind, "", percentage));
    }
    let data = AppUsageData::from_iter([(ProviderId::Claude, usage.clone())]);
    assert!(low_keys(&data, enabled).is_empty());
    // A promoted session limit and the window it duplicates are one quota.
    usage.session = section(97.0, Some(HOUR));
    usage.limits[0].usage = section(97.0, Some(HOUR));
    let data = AppUsageData::from_iter([(ProviderId::Claude, usage)]);
    assert_eq!(low_keys(&data, enabled), ["claude.five_hour"]);
}

fn account(provider: ProviderId, id: &str, selected: bool, usage: UsageData) -> AccountUsage {
    AccountUsage {
        provider,
        profile: crate::accounts::AccountProfile {
            id: id.into(),
            name: id.into(),
            enabled: true,
            ..Default::default()
        },
        source_signature: String::new(),
        source_path: None,
        usage: Some(usage),
        error: None,
        selected,
    }
}

#[test]
fn every_account_counts_not_only_the_selected_one() {
    let enabled = ProviderSet::from_enabled([ProviderId::Claude, ProviderId::Codex]);
    let healthy = UsageData {
        session: section(10.0, Some(HOUR)),
        weekly: section(10.0, Some(DAY)),
        ..Default::default()
    };
    let mut low_weekly = healthy.clone();
    low_weekly.weekly = section(98.0, Some(DAY));
    let mut data = AppUsageData::from_iter([(ProviderId::Claude, healthy.clone())]);
    data.accounts = vec![
        account(ProviderId::Claude, "personal", true, healthy.clone()),
        account(ProviderId::Claude, "work", false, low_weekly),
    ];
    // The selected provider-level reading is not counted twice.
    assert_eq!(low_keys(&data, enabled), ["accounts.claude.work.weekly"]);
    // A Codex weekly-only response is keyed as the weekly window.
    data.accounts = vec![account(
        ProviderId::Codex,
        "work",
        true,
        UsageData {
            weekly: section(97.0, Some(DAY)),
            ..Default::default()
        },
    )];
    assert_eq!(low_keys(&data, enabled), ["accounts.codex.work.weekly"]);
}

#[test]
fn an_almost_spent_limit_is_flagged_and_holds_the_bar_open() {
    let theme = top_bar();
    let providers = [ProviderId::Claude, ProviderId::Codex];
    let data = codex_weekly_at(96.0);
    for dark in [false, true] {
        let base = runtime(&providers, 1920).with_system_dark(dark);
        let sounding = base.with_alarm(alarm(true, true));
        let ctx = context(&theme, Some(&data), sounding);
        assert_eq!(ctx.get("codex.weekly.alarm"), Some(1.0));
        assert_eq!(ctx.get("claude.weekly.alarm"), Some(0.0));
        assert_eq!(ctx.get("claude.model.fable.alarm"), Some(0.0));
        // A solid red pill with white text, a red bar, a red label.
        assert!(bounds(&theme, "codex-weekly-pill", &data, sounding).is_some());
        let red = rgb(if dark { "#F26B6B" } else { "#EF4444" });
        assert_eq!(
            layer_colour(&theme, "codex-weekly-pill", &data, sounding),
            red
        );
        assert_eq!(
            layer_colour(&theme, "codex-weekly-fill", &data, sounding),
            red
        );
        assert_eq!(
            layer_colour(&theme, "codex-weekly-value", &data, sounding),
            rgb("#FFFFFF")
        );
        assert_eq!(
            layer_colour(&theme, "codex-weekly-label", &data, sounding),
            rgb(if dark { "#FF8F8F" } else { "#B42318" })
        );
        assert!(surface_force_reveal(&theme, 0, Some(&data), sounding));

        // Snoozed: still flagged, no longer forced open.
        let snoozed = base.with_alarm(ThemeAlarm {
            enabled: true,
            active: false,
            snoozed: true,
        });
        assert_eq!(
            layer_colour(&theme, "codex-weekly-pill", &data, snoozed),
            red
        );
        assert!(!surface_force_reveal(&theme, 0, Some(&data), snoozed));

        // Turned off: an ordinary 90%+ limit, never forced open.
        let off = base.with_alarm(alarm(false, false));
        assert_eq!(
            context(&theme, Some(&data), off).get("codex.weekly.alarm"),
            Some(0.0)
        );
        assert_eq!(
            layer_colour(&theme, "codex-weekly-pill", &data, off),
            rgb(if dark { "#F26B6B1F" } else { "#FEF0EF" })
        );
        assert!(!surface_force_reveal(&theme, 0, Some(&data), off));
    }
    // Six percent left is only the 90%+ warning.
    let six = codex_weekly_at(94.0);
    let ctx = context(
        &theme,
        Some(&six),
        runtime(&providers, 1920).with_alarm(alarm(true, false)),
    );
    assert_eq!(ctx.get("codex.weekly.alarm"), Some(0.0));
}

#[test]
fn the_snooze_button_shows_and_takes_clicks_only_while_the_alarm_holds_the_bar() {
    let theme = top_bar();
    let providers = [ProviderId::Claude, ProviderId::Codex];
    let data = codex_weekly_at(96.0);
    let base = runtime(&providers, 1920);
    let sounding = base.with_alarm(alarm(true, true));
    let (x, y, w, h) = bounds(&theme, "snooze-button", &data, sounding).unwrap();
    let centre = (x + w / 2.0, y + h / 2.0);
    let hit = |runtime| hit_test_mouse_event(&theme, 0, centre.0, centre.1, Some(&data), runtime);
    assert_eq!(hit(sounding).as_deref(), Some("snooze-button"));
    assert_eq!(
        mouse_event_script(&theme, 0, "snooze-button", MouseEventKind::Click),
        Some("snooze_alarm()")
    );
    assert_eq!(
        execute_mouse_actions(
            &theme,
            0,
            "snooze-button",
            "snooze_alarm()",
            Some(&data),
            sounding,
            &mut HashMap::new(),
        )
        .unwrap(),
        [MouseActionEffect::SnoozeAlarm]
    );
    // Its icon and text sit on it.
    for part in ["snooze-button-icon", "snooze-button-text"] {
        let (px, py, pw, ph) = bounds(&theme, part, &data, sounding).unwrap();
        assert!(
            px >= x && px + pw <= x + w && py >= y && py + ph <= y + h,
            "{part}"
        );
    }
    // No alarm, a snoozed one, or the alarm turned off: no button to hit.
    let snoozed = ThemeAlarm {
        enabled: true,
        active: false,
        snoozed: true,
    };
    for quiet in [alarm(true, false), snoozed, alarm(false, false)] {
        let runtime = base.with_alarm(quiet);
        for id in ["snooze-button", "snooze-button-icon", "snooze-button-text"] {
            assert!(
                bounds(&theme, id, &data, runtime).is_none(),
                "{quiet:?} {id}"
            );
        }
        assert_ne!(hit(runtime).as_deref(), Some("snooze-button"), "{quiet:?}");
    }
}

/// Something the bar paints, in physical pixels: a text's ink or a filled
/// shape. Edges are half-open, so two marks that touch are 0 px apart.
#[derive(Clone, Debug)]
struct Mark {
    id: String,
    text: Option<String>,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl Mark {
    /// Clear pixels between two marks; negative when they overlap.
    fn gap(&self, other: &Mark) -> i32 {
        let across = (other.left - self.right).max(self.left - other.right);
        let down = (other.top - self.bottom).max(self.top - other.bottom);
        across.max(down)
    }

    fn within(&self, other: &Mark) -> bool {
        self.left >= other.left
            && self.top >= other.top
            && self.right <= other.right
            && self.bottom <= other.bottom
    }
}

impl std::fmt::Display for Mark {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.id)?;
        if let Some(text) = &self.text {
            write!(f, " \"{text}\"")?;
        }
        write!(
            f,
            " [{},{}..{},{}]",
            self.left, self.top, self.right, self.bottom
        )
    }
}

/// The shape a mark is designed to sit on: a percentage on its level pill,
/// a status text or dot on its status pill, a bar fill on its track, a
/// provider dot in its halo, the Snooze button's icon and text on the button.
fn backdrop(id: &str) -> Option<String> {
    if id.starts_with("status-") && !id.ends_with("-pill") {
        return Some(format!("{}-pill", id.trim_end_matches("-dot")));
    }
    if id.starts_with("snooze-button-") {
        return Some("snooze-button".into());
    }
    [("-value", "-pill"), ("-fill", "-track"), ("-dot", "-halo")]
        .into_iter()
        .find_map(|(mark, under)| id.strip_suffix(mark).map(|owner| format!("{owner}{under}")))
}

/// Everything the Top Bar paints at a scale, as the renderer composites it:
/// each text's ink inside its layer and every parent clip, and each filled
/// shape except the card and its drop. Also returns the texts drawn narrower
/// than with unlimited room, which were cut off or ellipsized.
fn painted_marks(
    theme: &ThemeDocument,
    data: &AppUsageData,
    runtime: ThemeRuntime,
    scale: f64,
) -> (Vec<Mark>, Vec<String>) {
    let surface = &theme.surfaces[0];
    let (width, height) = resolve_surface_content_size(theme, 0, Some(data), runtime);
    let canvas = Canvas {
        width,
        width_expression: Some(surface.width.clone()),
        height,
        height_expression: Some(surface.height.clone()),
        background: surface.background.canvas_paint(),
    };
    let context = DataContext::from_usage_with_runtime(Some(data), &canvas, runtime);
    let (objects, warnings) =
        resolve_objects_for(surface, &canvas, &surface.children, Some(data), runtime);
    assert!(warnings.is_empty(), "{warnings:?}");
    let physical = |logical: f64| (logical * scale).round() as i32;
    let (mut marks, mut cut) = (Vec::new(), Vec::new());
    for object in objects.iter().filter(|object| object.opacity > 0.0) {
        let id = object.source.id.clone();
        let (left, top) = (physical(object.x), physical(object.y));
        let width = (object.width * scale).round().max(1.0) as u32;
        let height = (object.height * scale).round().max(1.0) as u32;
        let local = context.clone().with_object(object);
        match (&object.source.content, &object.source.background) {
            (
                SceneContent::Text {
                    template,
                    font_family,
                    font_size,
                    weight,
                    rendering,
                    align,
                    ..
                },
                _,
            ) => {
                let text = format_template(template, &local);
                if text.is_empty() {
                    continue;
                }
                let size = (evaluate(&font_size.0, &local).unwrap() * scale).max(1.0);
                let draw = |width: u32, align: TextAlign| {
                    let mut pixels = vec![0u32; width as usize * height as usize];
                    render_text_mask(
                        &mut pixels,
                        width,
                        height,
                        &text,
                        font_family,
                        size,
                        weight.gdi_weight(),
                        *rendering,
                        1.0,
                        align,
                        Rgba {
                            r: 255,
                            g: 255,
                            b: 255,
                            a: 255,
                        },
                    );
                    pixels
                };
                let pixels = draw(width, *align);
                let mut ink: Option<Mark> = None;
                for y in 0..height {
                    for x in 0..width {
                        if pixels[(y * width + x) as usize] >> 24 == 0 {
                            continue;
                        }
                        let (px, py) = (left + x as i32, top + y as i32);
                        let centre = ((px as f64 + 0.5) / scale, (py as f64 + 0.5) / scale);
                        if object
                            .clip
                            .iter()
                            .any(|region| !point_in_clip(centre, *region))
                        {
                            continue;
                        }
                        let mark = ink.get_or_insert_with(|| Mark {
                            id: id.clone(),
                            text: Some(text.clone()),
                            left: px,
                            top: py,
                            right: px,
                            bottom: py,
                        });
                        mark.left = mark.left.min(px);
                        mark.top = mark.top.min(py);
                        mark.right = mark.right.max(px + 1);
                        mark.bottom = mark.bottom.max(py + 1);
                    }
                }
                let room = (2 * width).max(512);
                let unlimited = draw(room, TextAlign::Left);
                let columns: Vec<u32> = (0..room)
                    .filter(|x| (0..height).any(|y| unlimited[(y * room + x) as usize] >> 24 > 0))
                    .collect();
                let full = match (columns.first(), columns.last()) {
                    (Some(first), Some(last)) => (last - first + 1) as i32,
                    _ => 0,
                };
                match ink {
                    Some(mark) => {
                        let drawn = mark.right - mark.left;
                        if drawn < full - 1 {
                            cut.push(format!("{mark} is cut off: {drawn} of {full} px drawn"));
                        }
                        marks.push(mark);
                    }
                    None => cut.push(format!("{id} \"{text}\" draws nothing")),
                }
            }
            (_, LayerBackground::Colour { colour })
                if id != "card" && !id.starts_with("drop-") && colour.resolve(&local).a > 0 =>
            {
                marks.push(Mark {
                    id,
                    text: None,
                    left,
                    top,
                    right: left + width as i32,
                    bottom: top + height as i32,
                });
            }
            _ => {}
        }
    }
    (marks, cut)
}

/// Every pair of marks keeps `clearance` px apart unless one is designed to
/// sit on the other, and then it stays inside it; every mark stays inside
/// `inner`. Returns the problems and the tightest independent pair.
fn crowding(marks: &[Mark], inner: &Mark, clearance: i32) -> (Vec<String>, (i32, String)) {
    let mut problems = Vec::new();
    let mut tightest = (i32::MAX, String::new());
    for (index, mark) in marks.iter().enumerate() {
        if !mark.within(inner) {
            problems.push(format!("{mark} leaves the card's inner padding {inner}"));
        }
        for other in &marks[index + 1..] {
            let (on, under) = if backdrop(&mark.id).as_deref() == Some(other.id.as_str()) {
                (mark, other)
            } else if backdrop(&other.id).as_deref() == Some(mark.id.as_str()) {
                (other, mark)
            } else {
                let gap = mark.gap(other);
                if gap < clearance {
                    problems.push(format!(
                        "{mark} and {other} are {gap} px apart, need {clearance}"
                    ));
                }
                if gap < tightest.0 {
                    tightest = (gap, format!("{} ~ {}", mark.id, other.id));
                }
                continue;
            };
            if !on.within(under) {
                problems.push(format!("{on} spills out of {under}"));
            }
        }
    }
    (problems, tightest)
}

/// Usage at one level on every limit of every provider, each resetting in
/// 23h 59m, the widest countdown. Codex reports a 30-day window, the widest
/// label.
fn everything_at(percentage: f64) -> AppUsageData {
    let window = || section(percentage, Some(23 * HOUR + 59 * 60));
    let usage = |weekly_label: Option<&str>| UsageData {
        session: window(),
        weekly: window(),
        weekly_label: weekly_label.map(Into::into),
        ..Default::default()
    };
    let mut claude = usage(None);
    claude.limits.push(UsageLimit {
        key: "weekly_scoped_fable".into(),
        kind: "weekly_scoped".into(),
        label: "Fable".into(),
        model: Some("Fable".into()),
        usage: window(),
        ..Default::default()
    });
    let mut opencode = usage(None);
    opencode.monthly = Some(window());
    AppUsageData::from_iter([
        (ProviderId::Claude, claude),
        (ProviderId::Codex, usage(Some("30d"))),
        (ProviderId::Cursor, usage(Some("API"))),
        (ProviderId::Antigravity, usage(None)),
        (ProviderId::Copilot, usage(None)),
        (ProviderId::Grok, usage(None)),
        (ProviderId::OpenCode, opencode),
    ])
}

/// Lay the bar out in each language, state, provider set and scale and
/// collect everything that touches, leaves the card or is cut short. `quick`
/// trims the matrix for the all-languages run.
fn top_bar_problems(languages: &[LanguageId], quick: bool) -> (Vec<String>, u32) {
    let theme = top_bar();
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let longest = 23 * HOUR + 59 * 60 + 30;
    let mut calm = fixture(true);
    for provider in [
        ProviderId::Antigravity,
        ProviderId::Copilot,
        ProviderId::Grok,
        ProviderId::OpenCode,
    ] {
        calm.insert(
            provider,
            UsageData {
                session: section(12.0, Some(2 * HOUR)),
                weekly: section(34.0, Some(5 * DAY)),
                ..Default::default()
            },
        );
    }
    let stale_at = |percentage| {
        let mut stale = everything_at(percentage);
        for provider in ProviderId::ALL {
            let usage = stale.get(provider).unwrap().clone();
            stale.insert(
                provider,
                UsageData {
                    stale: true,
                    ..usage
                },
            );
        }
        stale
    };
    // (state, data, seconds since the last update, alarm sounding, loading, countdown)
    let states = [
        ("normal", calm.clone(), 20, false, false, false),
        ("amber", everything_at(75.0), longest, false, false, false),
        ("red", everything_at(93.0), longest, false, false, false),
        ("full", everything_at(100.0), longest, false, false, false),
        ("alarm", everything_at(96.0), 20, true, false, false),
        // The Snooze button beside the longest status pill.
        ("alarm-stale", stale_at(96.0), longest, true, false, false),
        ("stale", stale_at(93.0), longest, false, false, false),
        ("waiting", calm.clone(), 20, false, true, false),
        ("countdown", everything_at(0.0), longest, false, false, true),
        ("missing", calm.clone(), longest, false, false, false),
    ];
    // Four or more providers can squeeze the bar to the monitor's width,
    // shrinking every cell, so a label may be ellipsized there; nothing may
    // overlap anywhere. Geometry ignores the mode, so those bigger sets run
    // in light mode only.
    let sets: [&[ProviderId]; 6] = [
        &[ProviderId::Claude],
        &[ProviderId::Claude, ProviderId::Codex],
        &[ProviderId::Claude, ProviderId::Codex, ProviderId::Cursor],
        &[
            ProviderId::Antigravity,
            ProviderId::Grok,
            ProviderId::OpenCode,
        ],
        &[
            ProviderId::Claude,
            ProviderId::Codex,
            ProviderId::Cursor,
            ProviderId::OpenCode,
        ],
        &ProviderId::ALL,
    ];
    let (mut problems, mut squeezed_bars) = (Vec::new(), 0);
    for providers in sets
        .into_iter()
        .filter(|providers| !quick || providers.len() == 2 || providers.len() == 7)
    {
        let names: Vec<_> = providers.iter().map(|p| p.descriptor().key).collect();
        let last = *providers.last().unwrap();
        for (state, data, age, sounding, loading, countdown) in states.iter().filter(|state| {
            !quick
                || matches!(
                    state.0,
                    "normal" | "stale" | "alarm-stale" | "waiting" | "countdown"
                )
        }) {
            let data = if *state == "missing" {
                AppUsageData::from_iter(
                    data.iter()
                        .filter(|(provider, _)| *provider != last)
                        .map(|(provider, usage)| (provider, usage.clone())),
                )
            } else {
                data.clone()
            };
            let scales: &[f64] = if quick {
                &[1.0, 1.5]
            } else {
                &[1.0, 1.25, 1.5]
            };
            for &scale in scales {
                // A 1920 px wide monitor at this scale.
                let host_width = (1920.0 / scale) as u32;
                for (dark, &language) in [false, true]
                    .into_iter()
                    .filter(|dark| providers.len() <= 3 || !dark)
                    .flat_map(|dark| languages.iter().map(move |language| (dark, language)))
                {
                    let runtime = ThemeRuntime::from_providers(ProviderSet::from_enabled(
                        providers.iter().copied(),
                    ))
                    .with_nest(SurfaceNest::Floating)
                    .with_host_dimensions(host_width, 1080)
                    .with_updated_unix(Some(now - age))
                    .with_system_dark(dark)
                    .with_language(language)
                    .with_alarm(alarm(true, *sounding))
                    .with_countdown(*countdown);
                    let runtime = if *loading {
                        runtime.with_poll_state(false, false)
                    } else {
                        runtime
                    };
                    let label = format!(
                        "{}/{}/{state}/{}%/{}",
                        language.code(),
                        names.join("+"),
                        scale * 100.0,
                        if dark { "dark" } else { "light" }
                    );
                    let (width, _) = resolve_surface_size(&theme, 0, Some(&data), runtime);
                    let squeezed = width >= host_width - 16;
                    squeezed_bars += squeezed as u32;
                    if squeezed && providers.len() <= 3 {
                        problems.push(format!("{label}: squeezed to {width} px"));
                    }
                    let (marks, cut) = painted_marks(&theme, &data, runtime, scale);
                    let physical = |logical: f64| (logical * scale).round() as i32;
                    // Rounding to device pixels may move an edge by one.
                    let inner = Mark {
                        id: "inner".into(),
                        text: None,
                        left: physical(EDGE) - 1,
                        top: physical(6.0) - 1,
                        right: physical(width as f64 - EDGE) + 1,
                        bottom: physical(f64::from(BAR_HEIGHT) - 4.0 - 6.0) + 1,
                    };
                    let (crowded, tightest) = crowding(&marks, &inner, physical(2.0));
                    // The scan must find what it checks.
                    let status = marks
                        .iter()
                        .find(|mark| mark.id.starts_with("status-") && mark.text.is_some());
                    let status_pill = marks
                        .iter()
                        .find(|mark| mark.id.starts_with("status-") && mark.id.ends_with("-pill"));
                    let level_pills = marks
                        .iter()
                        .filter(|mark| {
                            !mark.id.starts_with("status-") && mark.id.ends_with("-pill")
                        })
                        .count();
                    let texts = marks.iter().filter(|mark| mark.text.is_some()).count();
                    let (Some(status), Some(status_pill)) = (status, status_pill) else {
                        problems.push(format!("{label}: no status pill found"));
                        continue;
                    };
                    if matches!(
                        *state,
                        "amber" | "red" | "full" | "alarm" | "alarm-stale" | "stale"
                    ) && level_pills < providers.len()
                    {
                        problems.push(format!("{label}: only {level_pills} level pills found"));
                    }
                    // The Snooze button, its icon and its text show only
                    // while the alarm sounds.
                    let snooze = marks
                        .iter()
                        .filter(|mark| mark.id.starts_with("snooze-button"))
                        .count();
                    if snooze != if *sounding { 3 } else { 0 } {
                        problems.push(format!("{label}: {snooze} Snooze button marks found"));
                    }
                    let clear = marks
                        .iter()
                        .filter(|mark| !mark.id.starts_with("status-"))
                        .map(|mark| status_pill.gap(mark))
                        .min()
                        .unwrap();
                    println!(
                        "{label}: {width} px{}, {} marks ({texts} texts, {level_pills} level \
                         pills); {status} clears everything by {clear} px; tightest {} at {} px",
                        if squeezed { " squeezed" } else { "" },
                        marks.len(),
                        tightest.1,
                        tightest.0
                    );
                    let cut = if squeezed { Vec::new() } else { cut };
                    problems.extend(
                        crowded
                            .into_iter()
                            .chain(cut)
                            .map(|problem| format!("{label}: {problem}")),
                    );
                }
            }
        }
    }
    (problems, squeezed_bars)
}

fn assert_no_top_bar_problems((problems, squeezed_bars): (Vec<String>, u32)) {
    for problem in &problems {
        println!("PROBLEM {problem}");
    }
    assert!(squeezed_bars > 0, "no squeezed bar was checked");
    assert!(
        problems.is_empty(),
        "{} problems, first: {}",
        problems.len(),
        problems.first().unwrap()
    );
}

#[test]
fn the_status_words_and_their_order_come_from_the_language() {
    let theme = top_bar();
    let data = fixture(true);
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let line = |language, updated: Option<u64>, name: &str| {
        let runtime = runtime(&[ProviderId::Claude], 1920)
            .with_language(language)
            .with_updated_unix(updated);
        context(&theme, Some(&data), runtime)
            .get_string(name)
            .unwrap()
            .to_string()
    };
    let language = |code: &str| {
        LanguageId::ALL
            .into_iter()
            .find(|language| language.code() == code)
            .unwrap()
    };
    let three_minutes = Some(now - 3 * 60 - 5);
    for (language, live, stale, words) in [
        (
            language("en"),
            "Live · updated 3m ago",
            "Stale · updated 3m ago",
            "Waiting for usage data",
        ),
        (
            language("ja"),
            "ライブ · 3分前に更新",
            "古い · 3分前に更新",
            "使用量を待っています",
        ),
        (
            language("de"),
            "Live · vor 3m aktualisiert",
            "Veraltet · vor 3m aktualisiert",
            "Warte auf Nutzungsdaten",
        ),
    ] {
        assert_eq!(
            line(language, three_minutes, "i18n.status_live_updated"),
            live
        );
        assert_eq!(
            line(language, three_minutes, "i18n.status_stale_updated"),
            stale
        );
        assert_eq!(line(language, None, "i18n.status_waiting"), words);
        // Without a known update time there is no age to show.
        assert_eq!(line(language, None, "i18n.status_live_updated"), "");
    }
    // Every shipped language words all five lines and keeps the age slot.
    for language in LanguageId::ALL {
        let strings = language.strings();
        for template in [strings.status_live_updated, strings.status_stale_updated] {
            assert_eq!(template.matches("{time}").count(), 1, "{}", language.code());
        }
    }
}

#[test]
fn no_text_or_pill_in_the_top_bar_touches_another_or_leaves_the_card() {
    assert_no_top_bar_problems(top_bar_problems(&[LanguageId::English], false));
}

#[test]
fn nothing_in_the_top_bar_overlaps_or_is_cut_short_in_any_shipped_language() {
    assert_eq!(
        LanguageId::ALL.len(),
        14,
        "a language was added: measure it"
    );
    assert_no_top_bar_problems(top_bar_problems(&LanguageId::ALL, true));
}

/// Writes approval screenshots from fixture data, never live credentials:
/// `CCUM_APPROVAL_DIR=approval cargo test approval_screenshots -- --ignored`.
#[test]
#[ignore = "writes PNG files; run on demand"]
fn approval_screenshots() {
    let directory = std::path::PathBuf::from(
        std::env::var_os("CCUM_APPROVAL_DIR").expect("set CCUM_APPROVAL_DIR"),
    );
    std::fs::create_dir_all(&directory).unwrap();
    let theme = top_bar();
    let scale = std::env::var("CCUM_APPROVAL_SCALE")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1.0);
    let pair = [ProviderId::Claude, ProviderId::Codex];
    let three = [ProviderId::Claude, ProviderId::Codex, ProviderId::Cursor];
    // Codex weekly at 93% shows red with two providers, 75% amber with three.
    // The alarm shot has Claude's five-hour window at 96% used.
    let mut alarming = fixture(true);
    alarming.insert(
        ProviderId::Claude,
        UsageData {
            session: section(96.0, Some(HOUR + 12 * 60)),
            ..alarming.get(ProviderId::Claude).unwrap().clone()
        },
    );
    for (mode, dark) in [("light", false), ("dark", true)] {
        let shots: [(String, &[ProviderId], AppUsageData, ThemeAlarm); 3] = [
            (
                format!("topbar-{mode}-2.png"),
                &pair,
                codex_weekly_at(93.0),
                alarm(true, false),
            ),
            (
                format!("topbar-{mode}-3.png"),
                &three,
                codex_weekly_at(75.0),
                alarm(true, false),
            ),
            (
                format!("topbar-alarm-{mode}.png"),
                &pair,
                alarming.clone(),
                alarm(true, true),
            ),
        ];
        for (name, providers, data, alarm) in shots {
            let runtime = runtime(providers, 1920)
                .with_system_dark(dark)
                .with_alarm(alarm);
            let rendered =
                render_theme_surface_with_runtime_at_scale(&theme, 0, Some(&data), runtime, scale);
            assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
            save_on_backdrop(
                &rendered,
                (60.0 * scale) as u32,
                dark,
                &directory.join(name),
            );
        }
        let runtime = runtime(&pair, 1920).with_system_dark(dark);
        let handle = render_surface_handle(
            &theme,
            0,
            Some(&codex_weekly_at(75.0)),
            runtime,
            scale,
            SurfaceEdge::Top,
        );
        save_on_backdrop(
            &handle,
            (120.0 * scale) as u32,
            dark,
            &directory.join(format!("topbar-collapsed-{mode}.png")),
        );
    }
    // The spot with the longest status: Codex weekly at 93% under
    // "Stale · updated 23h 59m ago".
    let mut stale = codex_weekly_at(93.0);
    let claude = stale.get(ProviderId::Claude).unwrap().clone();
    stale.insert(
        ProviderId::Claude,
        UsageData {
            stale: true,
            ..claude
        },
    );
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let runtime = runtime(&pair, 1920)
        .with_system_dark(true)
        .with_updated_unix(Some(now - (23 * HOUR + 59 * 60 + 30)));
    let rendered =
        render_theme_surface_with_runtime_at_scale(&theme, 0, Some(&stale), runtime, scale);
    assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
    save_on_backdrop(
        &rendered,
        (60.0 * scale) as u32,
        true,
        &directory.join("topbar-dark-2-stale.png"),
    );
}

/// Composite premultiplied pixels at the top of a desktop-coloured backdrop
/// for the mode, with a strip of the opposite tone on the right so the edges
/// read true on both.
fn save_on_backdrop(rendered: &RenderedTheme, pad: u32, dark: bool, path: &std::path::Path) {
    let (width, height) = (rendered.width + 2 * pad, rendered.height + pad);
    let mut image = image::RgbaImage::new(width, height);
    let (main, other) = if dark {
        ([0x0F, 0x0F, 0x11], [0xF0, 0xF0, 0xF0])
    } else {
        ([0xF0, 0xF0, 0xF0], [0x0F, 0x0F, 0x11])
    };
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let backdrop: [u32; 3] = if x < width * 3 / 4 { main } else { other };
        let source = if x >= pad && x < pad + rendered.width && y < rendered.height {
            rendered.pixels[(y * rendered.width + x - pad) as usize]
        } else {
            0
        };
        let alpha = source >> 24;
        let channel = |shift: u32, base: u32| {
            ((source >> shift & 0xff) + base * (255 - alpha) / 255).min(255) as u8
        };
        *pixel = image::Rgba([
            channel(16, backdrop[0]),
            channel(8, backdrop[1]),
            channel(0, backdrop[2]),
            255,
        ]);
    }
    image.save(path).unwrap();
}
