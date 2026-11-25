use super::*;
use std::collections::BTreeSet;

fn e(source: &str) -> Expression {
    Expression(source.into())
}

fn paint(color: &str) -> Paint {
    Paint {
        color: color.into(),
        light_color: Some("#202020".into()),
        opacity: e("0.5"),
    }
}

/// A layer with every object field set to something other than its default.
/// Struct literals without `..Default::default()` make adding a field to
/// `SceneObject` fail to compile here until the test, and so
/// `OBJECT_FIELDS`, is updated.
fn layer(id: &str, background: LayerBackground, content: SceneContent) -> SceneObject {
    SceneObject {
        id: id.into(),
        name: id.into(),
        render: e("claude.available"),
        visibility: e("80"),
        parent: Some("main".into()),
        placement: Placement::default(),
        anchor: ObjectAnchor {
            horizontal: ObjectHorizontalAnchor::Right,
            vertical: ObjectVerticalAnchor::Bottom,
        },
        x: e("4"),
        y: e("2"),
        width: e("parent.width - 8"),
        height: e("12"),
        rotation: e("5"),
        background,
        border: Some(Stroke {
            color: paint("#FFFFFF"),
            width: e("1"),
        }),
        mouse_events: Some(MouseEvents {
            click: "show_dashboard()".into(),
            double_click: "toggle_dashboard()".into(),
            right_click: "show_context_menu()".into(),
            mouse_enter: "set(self, visibility, 100)".into(),
            mouse_leave: "reset(self, visibility)".into(),
        }),
        tooltip: Some(SurfaceTooltip::UsageSummary),
        corner_radius: e("3"),
        layout: ChildLayout::Row,
        align: ChildAlignment::Center,
        gap: e("2"),
        content,
        children: Vec::new(),
    }
}

fn maximal_theme() -> serde_json::Value {
    let layers = vec![
        layer(
            "text",
            LayerBackground::Colour {
                colour: paint("#101010"),
            },
            SceneContent::Text {
                template: "{claude.session.percentage:0}%".into(),
                font_family: "Segoe UI".into(),
                font_size: e("12"),
                weight: FontWeight::Bold,
                rendering: FontRendering::ClearType,
                contrast: e("1.2"),
                align: TextAlign::Center,
                color: paint("#FFFFFF"),
            },
        ),
        layer(
            "bar",
            LayerBackground::Gradient {
                start: paint("#000000"),
                end: paint("#FFFFFF"),
                angle: e("90"),
            },
            SceneContent::Progress {
                value: e("claude.session.display"),
                direction: ProgressDirection::RightToLeft,
                fill: paint("#46A758"),
                track: paint("#333333"),
                corner_radius: e("2"),
                segments: 4,
                segments_expression: Some(e("4")),
                segment_gap: e("1"),
            },
        ),
        layer(
            "mark",
            LayerBackground::Image {
                path: "logo.png".into(),
                fit: ImageFit::Cover,
            },
            SceneContent::ProviderMark {
                provider: "codex".into(),
                color: paint("#FFFFFF"),
            },
        ),
        layer("empty", LayerBackground::None, SceneContent::None),
    ];
    let mut surface = layer("main", LayerBackground::None, SceneContent::None);
    surface.parent = None;
    // Surfaces are placed on screen, not anchored in a parent.
    surface.anchor = ObjectAnchor::default();
    surface.x = e("0");
    surface.y = e("0");
    surface.children = layers;
    surface.placement = Placement {
        host_dimensions: None,
        clamp_taskbar_drag: false,
        reference: ReferenceTarget {
            region: ReferenceRegion::Monitor,
            display: 1,
        },
        nest: SurfaceNest::Floating,
        horizontal: HorizontalAnchor::Right,
        vertical: VerticalAnchor::Bottom,
        surface_horizontal: Some(HorizontalAnchor::Center),
        surface_vertical: Some(VerticalAnchor::Center),
        offset_x: 3,
        offset_x_expression: Some(e("host.width / 2")),
        offset_y: 4,
        offset_y_expression: Some(e("8")),
    };
    let document = ThemeDocument {
        schema_version: THEME_SCHEMA_VERSION,
        id: "maximal".into(),
        name: "Maximal".into(),
        canvas: Canvas::default(),
        placement: Placement::default(),
        children: Vec::new(),
        surfaces: vec![surface],
    };
    serde_json::to_value(&document).expect("a theme serializes")
}

#[test]
fn field_tables_list_exactly_what_themes_save() {
    let json = maximal_theme();
    let mut seen = BTreeSet::new();
    let mut unknown = Vec::new();
    visit_theme_fields(&json, &mut |owner, path, _, found| match found {
        Some(field) => {
            seen.insert(field.path);
        }
        None => unknown.push(format!("{owner}: {path}")),
    });
    assert!(
        unknown.is_empty(),
        "saved theme fields missing from DOCUMENT_FIELDS, OBJECT_FIELDS or PAINT_FIELDS \
         in theme_language.rs: {unknown:?}"
    );
    let missing: Vec<_> = DOCUMENT_FIELDS
        .iter()
        .chain(OBJECT_FIELDS)
        .chain(PAINT_FIELDS)
        .map(|field| field.path)
        .filter(|path| !seen.contains(path))
        .collect();
    assert!(
        missing.is_empty(),
        "listed fields a theme never saves; remove them or set them in maximal_theme: {missing:?}"
    );
}

/// The words serde accepts for a choice field, read from its error.
fn accepted_words(json: &serde_json::Value) -> Vec<String> {
    let error = match serde_json::from_value::<ThemeDocument>(json.clone()) {
        Ok(_) => return Vec::new(),
        Err(error) => error.to_string(),
    };
    let expected = error.split("expected").nth(1).unwrap_or_default();
    expected
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

fn set_path(json: &mut serde_json::Value, object: usize, path: &str, value: serde_json::Value) {
    let target = if object == 0 {
        &mut json["surfaces"][0]
    } else {
        &mut json["surfaces"][0]["children"][object - 1]
    };
    let mut target = target;
    let segments: Vec<&str> = path.split('.').collect();
    for segment in &segments[..segments.len() - 1] {
        let key = segment.split('(').next().unwrap_or(segment);
        target = &mut target[key];
    }
    let last = segments[segments.len() - 1];
    target[last.split('(').next().unwrap_or(last)] = value;
}

/// Which object in `maximal_theme` has a field: 0 is the surface, 1.. the layers.
fn object_with(path: &str) -> usize {
    match path.split('.').next().unwrap_or(path) {
        "content(text)" | "background(colour)" => 1,
        "content(progress)" | "background(gradient)" => 2,
        "content(provider_mark)" | "background(image)" => 3,
        _ => 0,
    }
}

#[test]
fn choice_fields_list_every_accepted_word() {
    let base = maximal_theme();
    for field in OBJECT_FIELDS {
        let FieldKind::Choice(words) = field.kind else {
            continue;
        };
        let mut json = base.clone();
        set_path(
            &mut json,
            object_with(field.path),
            field.path,
            "bogus".into(),
        );
        let accepted = accepted_words(&json);
        let listed: Vec<String> = words.iter().map(|word| (*word).to_string()).collect();
        assert_eq!(
            accepted, listed,
            "{} in OBJECT_FIELDS must list the words serde accepts, in order",
            field.path
        );
    }
}

#[test]
fn expression_and_template_fields_are_checked() {
    let base = maximal_theme();
    let mut unchecked = Vec::new();
    for field in OBJECT_FIELDS.iter().chain(PAINT_FIELDS) {
        let value = match field.kind {
            FieldKind::Expression | FieldKind::Condition => "bogus_name_xyz",
            FieldKind::Template => "{bogus_name_xyz}",
            _ => continue,
        };
        let paths: Vec<String> = if PAINT_FIELDS.iter().any(|paint| paint.path == field.path) {
            vec!["content(text).color.opacity".into()]
        } else {
            vec![field.path.to_string()]
        };
        for path in paths {
            let mut json = base.clone();
            let object = object_with(&path);
            // Layers do not have placement; set it on the surface.
            set_path(&mut json, object, &path, value.into());
            let theme: ThemeDocument = match serde_json::from_value(json) {
                Ok(theme) => theme,
                Err(error) => panic!("{path}: {error}"),
            };
            let errors = theme.validate();
            if !errors.iter().any(|error| error.contains("bogus_name_xyz")) {
                unchecked.push(format!("{path} ({:?})", field.kind));
            }
        }
    }
    assert!(
        unchecked.is_empty(),
        "fields listed as expressions or templates that validate() does not check: {unchecked:?}"
    );
}

#[test]
fn integer_fields_reject_expressions() {
    let base = maximal_theme();
    for field in OBJECT_FIELDS {
        if field.kind != FieldKind::Integer {
            continue;
        }
        let mut json = base.clone();
        set_path(
            &mut json,
            object_with(field.path),
            field.path,
            "1 + 1".into(),
        );
        assert!(
            serde_json::from_value::<ThemeDocument>(json).is_err(),
            "{} is listed as an integer but accepts an expression; list it as an expression",
            field.path
        );
    }
}

#[test]
fn unknown_fields_are_reported_with_a_suggestion() {
    let mut json = maximal_theme();
    json["surfaces"][0]["children"][0]["widht"] = "10".into();
    json["surfaces"][0]["children"][0]["content"]["font_sise"] = "10".into();
    json["surfaces"][0]["children"][1]["content"]["fill"]["colour"] = "#FFFFFF".into();
    json["surfaces"][0]["placement"]["horizontl"] = "left".into();
    let problems = unknown_theme_fields(&json);
    let expected = [
        "text: 'widht' is not a theme field; did you mean 'width'?",
        "text: 'content(text).font_sise' is not a theme field; did you mean 'content(text).font_size'?",
        "bar: 'content(progress).fill.colour' is not a theme field; did you mean 'color'?",
        "main: 'placement.horizontl' is not a theme field; did you mean 'placement.horizontal'?",
    ];
    for expected in expected {
        assert!(
            problems.iter().any(|problem| problem.starts_with(expected)),
            "missing {expected:?} in {problems:?}"
        );
    }
    assert_eq!(problems.len(), expected.len(), "{problems:?}");
    assert!(problems[0].contains(THEME_REFERENCE));
}

#[test]
fn legacy_fields_are_not_reported() {
    let json = serde_json::json!({
        "schema_version": 1, "id": "old", "name": "Old",
        "surfaces": [{"id": "main", "name": "Main", "placement": {"layer": "taskbar"},
            "content": {"type": "shape", "fill": {"color": "#FFFFFF"}}}]
    });
    assert_eq!(unknown_theme_fields(&json), Vec::<String>::new());
}

#[test]
fn every_listed_function_runs_with_its_argument_count() {
    for function in EXPRESSION_FUNCTIONS {
        if function.name == "get" {
            // `get` reads a value by name, so the parser handles it.
            continue;
        }
        let args = vec![ExpressionValue::Number(1.0); function.arguments];
        if let Err(error) = super::super::theme_expression::call_function(function.name, &args) {
            panic!("{} is listed but does not run: {error}", function.name);
        }
        let wrong = vec![ExpressionValue::Number(1.0); function.arguments + 1];
        let error = super::super::theme_expression::call_function(function.name, &wrong)
            .expect_err("too many arguments fail");
        assert!(error.contains(function.signature), "{error}");
    }
}

#[test]
fn unknown_names_suggest_the_closest_known_name() {
    let mut context = DataContext::from_usage(None, &Canvas::default());
    context.insert("claude.session.percentage", 42.0);

    let error = evaluate("clmap(1, 0, 2)", &context).expect_err("unknown function");
    assert!(error.contains("Did you mean 'clamp'?"), "{error}");

    let error = parse_mouse_actions("toggle_dashbord()").expect_err("unknown action");
    assert!(
        error.contains("Did you mean 'toggle_dashboard'?"),
        "{error}"
    );

    let errors = validate_template("{claude.sesion.percentage:0}", &context);
    assert!(
        errors[0].contains("did you mean 'claude.session.percentage'?")
            && errors[0].contains(THEME_REFERENCE),
        "{errors:?}"
    );

    let errors = validate_template("{claude.session.percentage:percnt}", &context);
    assert!(errors[0].contains("did you mean 'percent'?"), "{errors:?}");
}

#[test]
fn every_listed_format_is_accepted() {
    let mut context = DataContext::from_usage(None, &Canvas::default());
    context.insert("claude.session.percentage", 42.0);
    for format in TEMPLATE_FORMATS {
        for code in std::iter::once(format.code).chain(format.aliases.iter().copied()) {
            let code = if format.kind == FormatKind::UtcPrefix {
                "utc_iso_date".to_string()
            } else {
                code.to_string()
            };
            assert_eq!(unknown_format_message(&code), None, "{code}");
            if format.kind == FormatKind::Timestamp {
                assert!(
                    super::super::theme_datetime::format_timestamp(1.8e9, &code, &context)
                        .is_some(),
                    "{code} is listed as a timestamp format but format_timestamp ignores it"
                );
            }
        }
    }
    for number in ["0", "0.000", "#,##0.0"] {
        assert_eq!(unknown_format_message(number), None, "{number}");
    }
}

#[test]
fn every_action_example_parses_as_that_action() {
    for action in MOUSE_ACTIONS {
        assert!(
            action.example.starts_with(action.name) && action.signature.starts_with(action.name),
            "{}",
            action.name
        );
        if let Err(error) = parse_mouse_actions(action.example) {
            panic!("{} example fails: {error}", action.name);
        }
    }
}

#[test]
fn closest_only_suggests_likely_typos() {
    assert_eq!(
        closest("widht", ["width", "height"].into_iter()),
        Some("width")
    );
    assert_eq!(closest("colour", ["color"].into_iter()), Some("color"));
    assert_eq!(closest("banana", ["width", "height"].into_iter()), None);
}
