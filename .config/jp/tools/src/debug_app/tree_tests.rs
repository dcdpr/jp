use super::{Options, TreeNode, args, render};

/// A tree shaped like the app's: a window, a sidebar holding rows, and a
/// transcript.
/// Trimmed to the keys the renderer reads.
const TREE: &str = r#"{
  "role": "AXApplication",
  "actions": [],
  "children": [
    {
      "role": "AXWindow",
      "label": "mac-app",
      "actions": ["AXRaise"],
      "children": [
        {
          "role": "AXTable",
          "identifier": "sidebar.conversations",
          "label": "Conversations",
          "actions": [],
          "children": [
            {
              "role": "AXRow",
              "identifier": "jp-c12345",
              "label": "A conversation, 12 events",
              "enabled": true,
              "focused": true,
              "actions": ["AXPress"],
              "children": []
            }
          ],
          "elided_children": 411
        },
        {
          "role": "AXButton",
          "identifier": "transcript.copy",
          "label": "Copy Link",
          "enabled": false,
          "actions": ["AXPress"],
          "children": []
        }
      ]
    }
  ]
}"#;

fn tree() -> TreeNode {
    serde_json::from_str(TREE).unwrap()
}

/// The rendering is the diff surface, so it is pinned exactly rather than
/// spot-checked: a stray blank line or a reordered marker would make every
/// comparison noisy.
///
/// The fixture carries actions because it models a read that asked for them.
/// Whether to read them at all is the driver's decision now, so everything that
/// arrives is shown.
#[test]
fn renders_one_line_per_element() {
    let mut out = String::new();
    render(&tree(), 0, &Options::default(), &mut out);

    assert_eq!(
        out,
        "AXApplication\n  AXWindow \"mac-app\" (AXRaise)\n    AXTable #sidebar.conversations \
         \"Conversations\" (+411 not shown)\n      AXRow #jp-c12345 \"A conversation, 12 events\" \
         [focused] (AXPress)\n    AXButton #transcript.copy \"Copy Link\" [disabled] (AXPress)\n"
    );
}

/// A default read asks for no actions, so the driver reports none and there is
/// nothing to leave out.
#[test]
fn renders_no_actions_when_the_driver_reported_none() {
    let node: TreeNode = serde_json::from_str(
        r#"{"role": "AXButton", "identifier": "transcript.copy", "actions": [], "children": []}"#,
    )
    .unwrap();

    let mut out = String::new();
    render(&node, 0, &Options::default(), &mut out);

    assert_eq!(out, "AXButton #transcript.copy\n");
}

#[test]
fn renders_a_value_and_a_frame() {
    let node: TreeNode = serde_json::from_str(
        r#"{"role": "AXTextField", "value": "query", "frame": "0,0 100x20", "actions": [], "children": []}"#,
    )
    .unwrap();

    let opts = Options {
        frames: true,
        ..Options::default()
    };
    let mut out = String::new();
    render(&node, 0, &opts, &mut out);

    assert_eq!(out, "AXTextField = \"query\" @0,0 100x20\n");
}

/// Selecting a conversation has to be visible in a diff of two renderings,
/// which is the whole point of one line per element.
#[test]
fn a_selection_change_moves_one_line() {
    let before: TreeNode = serde_json::from_str(
        r#"{"role": "AXRow", "identifier": "a", "focused": true, "actions": [], "children": []}"#,
    )
    .unwrap();
    let after: TreeNode = serde_json::from_str(
        r#"{"role": "AXRow", "identifier": "a", "focused": false, "actions": [], "children": []}"#,
    )
    .unwrap();

    let mut first = String::new();
    render(&before, 0, &Options::default(), &mut first);
    let mut second = String::new();
    render(&after, 0, &Options::default(), &mut second);

    assert_eq!(first, "AXRow #a [focused]\n");
    assert_eq!(second, "AXRow #a\n");
}

/// Left unwalked, the Apple menu, Services, and the window tiling submenus run
/// to some two hundred lines around the handful describing the window.
///
/// The driver stops before reading them, so they arrive as a count rather than
/// as elements.
/// The note says so, because this is the one elision a caller can undo.
#[test]
fn names_the_menus_the_driver_left_unwalked() {
    let node: TreeNode = serde_json::from_str(
        r#"{
          "role": "AXApplication",
          "actions": [],
          "children": [
            {"role": "AXWindow", "label": "mac-app", "actions": [], "children": []},
            {"role": "AXMenuBar", "actions": [], "children": [], "elided_children": 2}
          ]
        }"#,
    )
    .unwrap();

    let mut out = String::new();
    render(&node, 0, &Options::default(), &mut out);

    assert_eq!(
        out,
        "AXApplication\n  AXWindow \"mac-app\"\n  AXMenuBar (2 menus not walked, pass `menus` for \
         them)\n"
    );
}

/// Asked for, the driver walks them and they render like anything else.
#[test]
fn renders_the_menus_when_the_driver_walked_them() {
    let node: TreeNode = serde_json::from_str(
        r#"{
          "role": "AXMenuBar",
          "actions": [],
          "children": [
            {"role": "AXMenuBarItem", "label": "Apple", "actions": [], "children": []},
            {"role": "AXMenuBarItem", "label": "File", "actions": [], "children": []}
          ]
        }"#,
    )
    .unwrap();

    let opts = Options {
        menus: true,
        ..Options::default()
    };
    let mut out = String::new();
    render(&node, 0, &opts, &mut out);

    assert_eq!(
        out,
        "AXMenuBar\n  AXMenuBarItem \"Apple\"\n  AXMenuBarItem \"File\"\n"
    );
}

/// An app with no menu bar of its own, or one read through a filter that pruned
/// it, must not grow a misleading note.
#[test]
fn says_nothing_about_an_empty_menu_bar() {
    let node: TreeNode =
        serde_json::from_str(r#"{"role": "AXMenuBar", "actions": [], "children": []}"#).unwrap();

    let mut out = String::new();
    render(&node, 0, &Options::default(), &mut out);

    assert_eq!(out, "AXMenuBar\n");
}

#[test]
fn args_default_to_reading_every_sibling() {
    assert_eq!(args(4321, &Options::default()), vec![
        "tree",
        "--pid",
        "4321",
        "--max-siblings",
        "0",
    ]);
}

#[test]
fn args_carry_every_option() {
    let opts = Options {
        identifier: Some("sidebar.".to_owned()),
        max_matches: Some(3),
        depth: Some(12),
        max_siblings: 5,
        frames: true,
        actions: false,
        menus: false,
    };

    assert_eq!(args(4321, &opts), vec![
        "tree",
        "--pid",
        "4321",
        "--max-siblings",
        "5",
        "--identifier",
        "sidebar.",
        "--max-matches",
        "3",
        "--depth",
        "12",
        "--frames",
    ]);
}

/// Both cost the driver work it would otherwise do and throw away: actions are
/// an accessibility round-trip per kept element, and the menu bar is a couple
/// of hundred elements belonging to macOS rather than to the app.
///
/// Filtering them out of the answer, as this once did, pays for them anyway.
#[test]
fn args_ask_for_actions_and_menus_rather_than_dropping_them_after() {
    let opts = Options {
        actions: true,
        menus: true,
        ..Options::default()
    };

    let asked = args(4321, &opts);

    assert!(asked.contains(&"--actions".to_owned()), "{asked:?}");
    assert!(asked.contains(&"--menus".to_owned()), "{asked:?}");

    let default = args(4321, &Options::default());

    assert!(!default.contains(&"--actions".to_owned()), "{default:?}");
    assert!(!default.contains(&"--menus".to_owned()), "{default:?}");
}
