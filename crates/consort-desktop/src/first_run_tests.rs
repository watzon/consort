//! The first conversation in the app: the worker card, the key field of the
//! first connect card, T8 and the one offer of the notification permission.

use consort_client::HubCardView;
use consort_client::mock_first::screen_state;
use gpui::{Entity, TestAppContext, VisualTestContext};

use crate::app::ConsortApp;
use crate::test_support::{click, draw, has, open_sized, settle};
use crate::ui::first_run::pending_card;

fn open_first(
    screen: &str,
    change: impl FnOnce(&mut consort_client::ViewState),
    cx: &mut TestAppContext,
) -> (VisualTestContext, Entity<ConsortApp>) {
    let mut state = screen_state(screen).expect("a state");
    change(&mut state);
    let (handle, app) = open_sized(state, 1440., 900., cx);
    let mut cx = VisualTestContext::from_window(handle, cx);
    settle(&mut cx, 50);
    cx.update(|window, cx| {
        app.update(cx, |app, cx| app.open_screen(screen, window, cx));
    });
    settle(&mut cx, 50);
    draw(&mut cx);
    (cx, app)
}

fn pending(app: &ConsortApp) -> Option<consort_client::ApprovalView> {
    pending_card(&app.state, &app.channel_id, |_: &HubCardView| true)
}

#[gpui::test]
fn the_worker_card_makes_one_code_for_each_card(cx: &mut TestAppContext) {
    let (mut cx, app) = open_first("first-t4", |s| s.worker_pairing = None, cx);
    assert!(has(&mut cx, "worker-card"));
    app.read_with(&cx, |app, _| assert!(app.state.worker_pairing.is_none()));
    cx.update(|_, cx| app.update(cx, |app, cx| app.follow_first_run(cx)));
    settle(&mut cx, 50);
    app.read_with(&cx, |app, _| {
        assert!(app.state.worker_pairing.is_some(), "the card has its code");
    });
    // The next state makes no second code for the same card.
    cx.update(|_, cx| {
        app.update(cx, |app, cx| {
            std::sync::Arc::make_mut(&mut app.state).worker_pairing = None;
            app.follow_first_run(cx);
        })
    });
    settle(&mut cx, 20);
    app.read_with(&cx, |app, _| {
        assert!(app.state.worker_pairing.is_none());
        assert!(app.ext.worker_card.is_some());
    });
}

#[gpui::test]
fn not_now_answers_the_worker_card(cx: &mut TestAppContext) {
    let (mut cx, app) = open_first("first-t4", |_| {}, cx);
    click(&mut cx, "worker-card-not-now");
    settle(&mut cx, 50);
    app.read_with(&cx, |app, _| assert!(pending(app).is_none()));
    assert!(!has(&mut cx, "worker-card"));
}

#[gpui::test]
fn an_expired_code_offers_a_new_code(cx: &mut TestAppContext) {
    let (mut cx, app) = open_first(
        "first-t4",
        |s| {
            if let Some(pairing) = s.worker_pairing.as_mut() {
                pairing.expires_at = 1;
            }
        },
        cx,
    );
    assert!(has(&mut cx, "worker-card-new-code"));
    click(&mut cx, "worker-card-new-code");
    settle(&mut cx, 50);
    app.read_with(&cx, |app, _| {
        let code = app.state.worker_pairing.as_ref().expect("a code");
        assert!(code.expires_at > 1);
    });
    assert!(!has(&mut cx, "worker-card-new-code"));
}

#[gpui::test]
fn the_first_connect_card_needs_the_key_and_clears_it(cx: &mut TestAppContext) {
    let (mut cx, app) = open_first("first-t3", |_| {}, cx);
    assert!(has(&mut cx, "card-composio-key"));
    let id = app
        .read_with(&cx, |app, _| pending(app))
        .expect("a card")
        .id;

    // No key: the card stays and says what to do.
    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.continue_connect_card_with_key(id.clone(), window, cx)
        })
    });
    settle(&mut cx, 20);
    app.read_with(&cx, |app, _| {
        assert!(pending(app).is_some());
        assert_eq!(
            app.key_error.as_deref(),
            Some("Paste your Composio project key.")
        );
    });

    cx.update(|window, cx| {
        app.update(cx, |app, cx| {
            app.ext_inputs.composio_key.update(cx, |input, cx| {
                input.set_value("ak_test_key", window, cx);
            });
            app.continue_connect_card_with_key(id.clone(), window, cx);
        })
    });
    settle(&mut cx, 50);
    app.read_with(&cx, |app, cx| {
        assert!(app.state.composio.key_present, "the hub has the key");
        assert_eq!(app.ext_inputs.composio_key.read(cx).value().as_ref(), "");
        assert!(pending(app).is_none(), "the card is answered");
        assert_eq!(
            app.state.connections.last().map(|c| c.name.as_str()),
            Some("Gmail")
        );
        // The view of the timeline holds no key.
        assert!(!format!("{:?}", app.state.timelines).contains("ak_test_key"));
    });
    assert!(!has(&mut cx, "card-composio-key"));
}

#[gpui::test]
fn a_hub_with_a_key_shows_the_usual_connect_card(cx: &mut TestAppContext) {
    let (mut cx, _app) = open_first("first-t3", |s| s.composio.key_present = true, cx);
    assert!(!has(&mut cx, "card-composio-key"));
}

#[gpui::test]
fn t8_has_the_words_of_the_design(cx: &mut TestAppContext) {
    let (mut cx, app) = open_first("first-t8", |_| {}, cx);
    assert!(has(&mut cx, "no-account-state"));
    assert!(has(&mut cx, "no-account-sign-in"));
    assert!(has(&mut cx, "setup-mark"));
    app.read_with(&cx, |app, _| {
        assert_eq!(
            app.composer_placeholder(),
            "Sign in to a provider to message the assistant"
        );
    });
}

#[gpui::test]
fn signed_out_with_messages_keeps_the_timeline(cx: &mut TestAppContext) {
    let (mut cx, _app) = open_first("first-signed-out", |_| {}, cx);
    assert!(!has(&mut cx, "no-account-state"));
    assert!(has(&mut cx, "no-account-card"));
}

#[gpui::test]
fn the_permission_offer_shows_once(cx: &mut TestAppContext) {
    let (mut cx, app) = open_first("first-permission", |_| {}, cx);
    assert!(has(&mut cx, "permission-offer"));
    click(&mut cx, "permission-offer-not-now");
    settle(&mut cx, 20);
    assert!(!has(&mut cx, "permission-offer"));
    app.read_with(&cx, |app, _| {
        assert!(app.admin.settings.permission_offered);
    });
}

#[gpui::test]
fn the_close_with_no_system_question_makes_no_offer(cx: &mut TestAppContext) {
    let (mut cx, _app) = open_first("first-t7", |_| {}, cx);
    assert!(!has(&mut cx, "permission-offer"));
}

#[gpui::test]
fn the_sign_in_line_has_no_time_of_its_own(cx: &mut TestAppContext) {
    let (mut cx, _app) = open_first("first-t1", |_| {}, cx);
    assert!(has(&mut cx, "notice-text-ntc_signed_in"));
    assert!(!has(&mut cx, "notice-time-ntc_signed_in"));
}
