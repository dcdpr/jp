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
#[test]
fn renders_one_line_per_element() {
    let mut out = String::new();
    render(&tree(), 0, &Options::default(), &mut out);

    assert_eq!(
        out,
        "AXApplication\n  AXWindow \"mac-app\"\n    AXTable #sidebar.conversations \
         \"Conversations\" (+411 not shown)\n      AXRow #jp-c12345 \"A conversation, 12 events\" \
         [focused]\n    AXButton #transcript.copy \"Copy Link\" [disabled]\n"
    );
}

#[test]
fn renders_actions_only_when_asked() {
    let opts = Options {
        actions: true,
        ..Options::default()
    };
    let mut out = String::new();
    render(&tree(), 0, &opts, &mut out);

    assert_eq!(
        out,
        "AXApplication\n  AXWindow \"mac-app\" (AXRaise)\n    AXTable #sidebar.conversations \
         \"Conversations\" (+411 not shown)\n      AXRow #jp-c12345 \"A conversation, 12 events\" \
         [focused] (AXPress)\n    AXButton #transcript.copy \"Copy Link\" [disabled] (AXPress)\n"
    );
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
#[test]
fn leaves_the_menu_bar_unwalked_by_default() {
    let node: TreeNode = serde_json::from_str(
        r#"{
          "role": "AXApplication",
          "actions": [],
          "children": [
            {"role": "AXWindow", "label": "mac-app", "actions": [], "children": []},
            {
              "role": "AXMenuBar",
              "actions": [],
              "children": [
                {"role": "AXMenuBarItem", "label": "Apple", "actions": [], "children": []},
                {"role": "AXMenuBarItem", "label": "File", "actions": [], "children": []}
              ]
            }
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

    let opts = Options {
        menus: true,
        ..Options::default()
    };
    let mut walked = String::new();
    render(&node, 0, &opts, &mut walked);
    assert_eq!(
        walked,
        "AXApplication\n  AXWindow \"mac-app\"\n  AXMenuBar\n    AXMenuBarItem \"Apple\"\n    \
         AXMenuBarItem \"File\"\n"
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
