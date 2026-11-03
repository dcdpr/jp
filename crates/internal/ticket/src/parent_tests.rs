use super::*;

fn id(id: &str) -> TicketId {
    id.parse().unwrap()
}

fn member(ticket: &str, status: Status, parent: Option<&str>) -> Member {
    Member {
        id: id(ticket),
        status,
        parent: parent.map(id),
    }
}

/// A parent, `T-0aaaaaa`, with two children, one of them Done, and a ticket
/// under nothing, `T-0dddddd`.
fn board(parent_status: Status) -> Vec<Member> {
    vec![
        member("T-0aaaaaa", parent_status, None),
        member("T-0bbbbbb", Status::Todo, Some("T-0aaaaaa")),
        member("T-0cccccc", Status::Done, Some("T-0aaaaaa")),
        member("T-0dddddd", Status::Todo, None),
    ]
}

#[test]
fn children_are_the_tickets_naming_the_parent_in_board_order() {
    assert_eq!(children(&board(Status::Todo), id("T-0aaaaaa")), vec![
        id("T-0bbbbbb"),
        id("T-0cccccc")
    ]);
    assert_eq!(children(&board(Status::Todo), id("T-0dddddd")), vec![]);
}

#[test]
fn a_new_ticket_can_be_filed_under_an_open_parent() {
    assert_eq!(
        check(
            &board(Status::Todo),
            None,
            Status::Todo,
            Some(id("T-0aaaaaa"))
        ),
        Ok(())
    );
}

#[test]
fn a_new_ticket_under_a_missing_parent_is_refused() {
    let refusal = check(
        &board(Status::Todo),
        None,
        Status::Todo,
        Some(id("T-0zzzzzz")),
    );

    assert_eq!(
        refusal.unwrap_err().to_string(),
        "No ticket T-0zzzzzz to file under."
    );
}

#[test]
fn a_new_ticket_under_a_done_parent_is_refused() {
    let refusal = check(
        &board(Status::Done),
        None,
        Status::Todo,
        Some(id("T-0aaaaaa")),
    );

    assert_eq!(
        refusal.unwrap_err().to_string(),
        "T-0aaaaaa is Done, so no open ticket can be filed under it. Reopen T-0aaaaaa first."
    );
}

#[test]
fn a_ticket_named_as_its_own_parent_is_refused() {
    let refusal = check(
        &board(Status::Todo),
        Some(id("T-0dddddd")),
        Status::Todo,
        Some(id("T-0dddddd")),
    );

    assert_eq!(refusal, Err(Refusal::Itself(id("T-0dddddd"))));
}

#[test]
fn a_parent_under_a_parent_is_refused_and_the_message_names_the_top() {
    let refusal = check(
        &board(Status::Todo),
        Some(id("T-0dddddd")),
        Status::Todo,
        Some(id("T-0bbbbbb")),
    );

    assert_eq!(
        refusal.unwrap_err().to_string(),
        "T-0bbbbbb sits under T-0aaaaaa, and a ticket under a parent can't be a parent itself. \
         File under T-0aaaaaa instead."
    );
}

#[test]
fn a_ticket_with_children_cannot_be_given_a_parent() {
    let refusal = check(
        &board(Status::Todo),
        Some(id("T-0aaaaaa")),
        Status::Todo,
        Some(id("T-0dddddd")),
    );

    assert_eq!(
        refusal,
        Err(Refusal::HasChildren {
            id: id("T-0aaaaaa"),
            children: vec![id("T-0bbbbbb"), id("T-0cccccc")],
        })
    );
}

#[test]
fn closing_a_parent_with_open_children_is_refused_naming_only_the_open_ones() {
    let refusal = check(
        &board(Status::Todo),
        Some(id("T-0aaaaaa")),
        Status::Done,
        None,
    );

    assert_eq!(
        refusal.unwrap_err().to_string(),
        "T-0aaaaaa has open children: T-0bbbbbb. Close them, or move them out from under \
         T-0aaaaaa, first."
    );
}

#[test]
fn closing_a_parent_whose_children_are_done_is_allowed() {
    let mut members = board(Status::Todo);
    members[1].status = Status::Done;

    assert_eq!(
        check(&members, Some(id("T-0aaaaaa")), Status::Done, None),
        Ok(())
    );
}

#[test]
fn reopening_a_child_of_a_done_parent_is_refused() {
    let mut members = board(Status::Done);
    members[1].status = Status::Done;

    let refusal = check(
        &members,
        Some(id("T-0cccccc")),
        Status::Todo,
        Some(id("T-0aaaaaa")),
    );

    assert_eq!(
        refusal.unwrap_err().to_string(),
        "T-0cccccc would be open under T-0aaaaaa, which is Done. Reopen T-0aaaaaa first."
    );
}

#[test]
fn a_change_that_leaves_parent_and_openness_alone_is_not_checked() {
    // Broken by hand: the parent is gone. Moving the child between open
    // columns touches neither, so it is not refused for it.
    let members = vec![member("T-0bbbbbb", Status::Todo, Some("T-0aaaaaa"))];

    assert_eq!(
        check(
            &members,
            Some(id("T-0bbbbbb")),
            Status::InProgress,
            Some(id("T-0aaaaaa"))
        ),
        Ok(())
    );
}

#[test]
fn clearing_a_parent_is_always_allowed() {
    assert_eq!(
        check(
            &board(Status::Done),
            Some(id("T-0bbbbbb")),
            Status::Todo,
            None
        ),
        Ok(())
    );
}

#[test]
fn deleting_a_parent_is_refused_and_a_childless_ticket_is_not() {
    assert_eq!(
        check_delete(&board(Status::Todo), id("T-0aaaaaa"))
            .unwrap_err()
            .to_string(),
        "T-0aaaaaa has children: T-0bbbbbb, T-0cccccc. Give them another parent, or none, before \
         deleting it."
    );
    assert_eq!(check_delete(&board(Status::Todo), id("T-0bbbbbb")), Ok(()));
}

#[test]
fn problems_name_every_broken_relation_once() {
    let members = vec![
        member("T-0aaaaaa", Status::Done, None),
        member("T-0bbbbbb", Status::Todo, Some("T-0aaaaaa")),
        member("T-0cccccc", Status::Todo, Some("T-0bbbbbb")),
        member("T-0dddddd", Status::Todo, Some("T-0zzzzzz")),
        member("T-0eeeeee", Status::Todo, Some("T-0eeeeee")),
    ];

    let messages: Vec<String> = problems(&members)
        .iter()
        .map(|(_, refusal)| refusal.to_string())
        .collect();

    assert_eq!(messages, vec![
        "T-0aaaaaa has open children: T-0bbbbbb. Close them, or move them out from under \
         T-0aaaaaa, first."
            .to_owned(),
        "T-0bbbbbb sits under T-0aaaaaa, and a ticket under a parent can't be a parent itself. \
         File under T-0aaaaaa instead."
            .to_owned(),
        "T-0dddddd names T-0zzzzzz as its parent, and there is no ticket T-0zzzzzz.".to_owned(),
        "T-0eeeeee can't be its own parent.".to_owned(),
    ]);
}

#[test]
fn a_board_without_parents_has_no_problems() {
    let members = vec![
        member("T-0aaaaaa", Status::Done, None),
        member("T-0bbbbbb", Status::Todo, None),
    ];

    assert_eq!(problems(&members), vec![]);
}
