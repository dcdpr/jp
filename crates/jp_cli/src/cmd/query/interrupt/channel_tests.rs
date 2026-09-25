use tokio::sync::oneshot::error::TryRecvError;

use super::*;

fn reply(content: &str) -> InterruptAction {
    InterruptAction::Reply {
        content: content.to_owned(),
        echo: true,
    }
}

/// Queued is not delivered: the sender hears nothing until the turn takes the
/// interrupt.
#[test]
fn an_interrupt_is_acknowledged_when_the_turn_takes_it() {
    let (tx, mut interrupts) = TurnInterrupts::channel();
    let mut delivery = tx.try_send(InterruptAction::Stop).unwrap();

    assert_eq!(delivery.try_recv(), Err(TryRecvError::Empty));

    assert_eq!(interrupts.try_next(), Some(InterruptAction::Stop));
    assert_eq!(delivery.try_recv(), Ok(()));
}

/// A turn that ends with an interrupt still queued refuses it, rather than
/// leaving the sender to believe it was acted on.
#[test]
fn an_interrupt_the_turn_never_took_is_refused() {
    let (tx, interrupts) = TurnInterrupts::channel();
    let mut delivery = tx.try_send(reply("use Rust")).unwrap();

    drop(interrupts);

    assert_eq!(delivery.try_recv(), Err(TryRecvError::Closed));
}

/// Before the turn starts, a stop is acted on and a reply is held for later, in
/// order, without being acknowledged.
#[tokio::test]
async fn a_reply_waits_out_the_preparation_and_a_stop_does_not() {
    let (tx, mut interrupts) = TurnInterrupts::channel();
    let mut first = tx.try_send(reply("first")).unwrap();
    let mut second = tx.try_send(reply("second")).unwrap();
    let mut stop = tx.try_send(InterruptAction::Stop).unwrap();

    assert_eq!(interrupts.next_stop().await, Some(InterruptAction::Stop));
    assert_eq!(stop.try_recv(), Ok(()));
    assert_eq!(first.try_recv(), Err(TryRecvError::Empty));
    assert_eq!(second.try_recv(), Err(TryRecvError::Empty));

    assert_eq!(interrupts.try_next(), Some(reply("first")));
    assert_eq!(first.try_recv(), Ok(()));
    assert_eq!(interrupts.next().await, Some(reply("second")));
    assert_eq!(second.try_recv(), Ok(()));
}

/// A held reply is refused like any other when the turn ends without it.
#[tokio::test]
async fn a_held_reply_is_refused_when_the_turn_does_not_start() {
    let (tx, mut interrupts) = TurnInterrupts::channel();
    let mut held = tx.try_send(reply("never read")).unwrap();
    drop(tx);

    // Every sender is gone, so the wait ends with the reply held.
    assert_eq!(interrupts.next_stop().await, None);
    drop(interrupts);

    assert_eq!(held.try_recv(), Err(TryRecvError::Closed));
}

#[test]
fn nothing_reaches_a_turn_driven_from_the_terminal() {
    let mut interrupts = TurnInterrupts::none();

    assert_eq!(interrupts.try_next(), None);
}
