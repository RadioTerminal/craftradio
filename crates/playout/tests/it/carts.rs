//! Carts over the air: a cart with a duck depth lowers the music while it plays.

use radio_engine::{CartMode, Command};
use radio_model::ItemId;

use crate::fixtures::{SR, Side, add_playlist, engine_with, file_item, offline, rms_db, run, wav};

#[test]
fn a_cart_with_a_duck_lowers_the_song_under_it() {
    let song = wav("cart-song", 8.0, Side::Left);
    let mut engine = engine_with(vec![file_item(1, "Long Field", &song, 8.0, 7.5, 0.0)]);
    let id = wav("cart-id", 1.5, Side::Right);
    add_playlist(
        &mut engine,
        "Carts",
        vec![file_item(20, "Station ID", &id, 1.5, 1.5, 0.0)],
        "",
    );
    engine
        .dispatch(Command::CartAssign {
            page: 0,
            slot: 0,
            item: ItemId(20),
        })
        .unwrap();
    engine
        .dispatch(Command::CartSet {
            page: 0,
            slot: 0,
            duck_db: -12.0,
            mode: CartMode::OneShot,
        })
        .unwrap();
    let (p, clock) = offline(engine, |_| {});
    let before = run(&p, &clock, 2.0);
    p.send(Command::CartFire { page: 0, slot: 0 }).unwrap();
    let during = run(&p, &clock, 1.0);
    let after = run(&p, &clock, 2.0);
    let song_level = |pcm: &[f32], a: f32, b: f32| {
        rms_db(pcm, 0, (a * SR as f32) as usize, (b * SR as f32) as usize)
    };
    let (b, d, a) = (
        song_level(&before, 1.0, 2.0),
        song_level(&during, 0.4, 0.9),
        song_level(&after, 1.0, 2.0),
    );
    assert!((b - d - 12.0).abs() < 2.0, "ducked by {} dB", b - d);
    assert!((b - a).abs() < 1.0, "came back {} dB below", b - a);
}
