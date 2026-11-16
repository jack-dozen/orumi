use super::*;

const A: WindowId = WindowId(1);
const B: WindowId = WindowId(2);
const C: WindowId = WindowId(3);
const X: WindowId = WindowId(4);
const D: WindowId = WindowId(5);

fn strip() -> Strip {
    Strip::new(Rect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    })
}

fn r(s: &Strip, id: WindowId) -> Rect {
    s.layout().into_iter().find(|p| p.id == id).unwrap().rect
}

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rect { x, y, w, h }
}

#[test]
fn new_window_goes_below_and_scrolls() {
    let mut s = strip();
    s.add_window(A);
    s.add_window(B);
    assert_eq!(s.focused(), Some(B));
    assert_eq!(r(&s, B).y, 0);
    assert_eq!(r(&s, A).y, -1080);
    let a = s.layout().into_iter().find(|p| p.id == A).unwrap();
    assert!(!a.visible);
}

#[test]
fn up_shrinks_and_down_grows() {
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    assert_eq!(r(&s, A).h, 540);
    s.snap(Dir::Up);
    assert_eq!(r(&s, A).h, 360);
    s.snap(Dir::Down);
    s.snap(Dir::Down);
    assert_eq!(r(&s, A).h, 1080);
}

#[test]
fn snap_left_gives_half_width_and_leaves_right_free() {
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    assert_eq!(r(&s, A), rect(0, 0, 960, 540));
}

#[test]
fn snapping_same_side_again_is_noop() {
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Left);
    let before = s.items().to_vec();
    s.snap(Dir::Left);
    assert_eq!(s.items(), &before[..]);
}

#[test]
fn opposite_side_goes_full_width_first_then_other_side() {
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.snap(Dir::Right); // lebar penuh dulu, tinggi tetap
    assert_eq!(r(&s, A), rect(0, 0, 1920, 540));
    s.snap(Dir::Right); // baru pindah ke kanan
    assert_eq!(r(&s, A), rect(960, 0, 960, 540));
}

#[test]
fn heights_are_independent_per_window() {
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(B);
    s.snap(Dir::Right); // B tetap setinggi penuh
    assert_eq!(r(&s, A), rect(0, 0, 960, 540));
    assert_eq!(r(&s, B), rect(960, 0, 960, 1080));
}

#[test]
fn swap_when_opposite_lane_is_free() {
    // A kuarter kiri atas, kanan kosong. B snap kiri -> B kiri atas, A kanan atas.
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(B);
    s.snap(Dir::Left);
    assert_eq!(r(&s, B), rect(0, 0, 960, 1080));
    assert_eq!(r(&s, A), rect(960, 0, 960, 540));
}

/// Y: separuh layar kanan (penuh tinggi). X: kuarter kiri atas.
fn y_right_x_top_left() -> Strip {
    let mut s = strip();
    s.add_window(A); // A = Y
    s.snap(Dir::Right);
    s.add_window(X);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s
}

#[test]
fn push_down_when_opposite_lane_is_occupied() {
    let mut s = y_right_x_top_left();
    assert_eq!(r(&s, X), rect(0, 0, 960, 540));
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    // B di kiri atas, X turun ke bawahnya, A (kanan penuh) tidak tergeser
    assert_eq!(r(&s, B), rect(0, 0, 960, 540));
    assert_eq!(r(&s, X), rect(0, 540, 960, 540));
    assert_eq!(r(&s, A), rect(960, 0, 960, 1080));
}

#[test]
fn maximize_restores_full_size() {
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.maximize();
    assert_eq!(r(&s, A), rect(0, 0, 1920, 1080));
}

#[test]
fn maximize_resets_the_lane_ratio_so_snapping_again_gives_half() {
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Left);
    assert!(s.adjust_split(96));
    assert_eq!(r(&s, A).w, 1056); // pembagi sudah digeser dari 1/2
    s.maximize();
    assert_eq!(r(&s, A), rect(0, 0, 1920, 1080)); // kembali semula
    s.snap(Dir::Left);
    assert_eq!(r(&s, A).w, 960); // 1/2 lagi, bukan rasio lama
}

#[test]
fn rejoining_a_block_after_maximize_still_follows_that_blocks_ratio() {
    let mut s = two_half_lanes();
    s.set_focus(A);
    assert!(s.adjust_split(96)); // blok A+B: kiri 1056, kanan 864
    s.set_focus(B);
    s.maximize();
    assert_eq!(r(&s, B).w, 1920);
    s.snap(Dir::Right); // masuk lagi ke blok yang punya pembagi: ikut rasionya
    assert_eq!(r(&s, B).w, 864);
    assert_eq!(r(&s, A).w, 1056);
}

#[test]
fn moving_a_full_width_window_leaves_a_cue_for_the_app() {
    let mut s = strip();
    s.add_window(A);
    s.add_window(B); // B fokus, di bawah A (keduanya penuh lebar)
    s.move_window(Dir::Up);
    assert_eq!(s.take_move_cue(), Some((B, Dir::Up)));
    assert_eq!(s.take_move_cue(), None); // sekali pakai
}

#[test]
fn lane_swaps_and_failed_moves_leave_no_cue() {
    let mut s = two_half_lanes();
    s.set_focus(A);
    s.move_window(Dir::Right); // tukar sisi: sudah kelihatan di layar
    assert_eq!(s.take_move_cue(), None);
    let mut s = strip();
    s.add_window(A);
    s.move_window(Dir::Down); // tidak ada tetangga
    assert_eq!(s.take_move_cue(), None);
}

#[test]
fn min_height_stretches_aligned_neighbor_to_keep_edges_even() {
    // A dan B sama-sama setengah tinggi, berdampingan. B punya minimum 600:
    // A diregangkan ke 600 supaya dasar keduanya rata.
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Right);
    assert!(s.set_min_height(B, 600));
    assert_eq!(r(&s, B).h, 600);
    assert_eq!(r(&s, A).h, 600);
    assert!(!s.set_min_height(B, 500));
}

#[test]
fn quarter_grid_rows_stay_aligned_with_min_height() {
    // 2x2: A B / C D. B punya tinggi minimum -> kedua baris tetap sejajar.
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Right);
    s.add_window(C);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(D);
    s.snap(Dir::Up);
    s.snap(Dir::Right);
    s.set_min_height(B, 600);
    let (a, b, c, d) = (r(&s, A), r(&s, B), r(&s, C), r(&s, D));
    assert_eq!(a.y, b.y);
    assert_eq!(a.h, b.h);
    assert_eq!(c.y, d.y);
    assert_eq!(c.y - a.y, 600);
    assert_eq!(c.y + c.h, d.y + d.h);
}

#[test]
fn new_window_goes_below_whole_block_without_pushing_neighbors() {
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Right);
    s.set_focus(A);
    s.add_window(C);
    assert_eq!(r(&s, B).y, r(&s, A).y); // B tidak turun
    assert_eq!(r(&s, C).y - r(&s, A).y, 540);
}

#[test]
fn min_height_pushes_window_below_in_same_lane() {
    let mut s = y_right_x_top_left();
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.set_focus(B);
    s.set_min_height(B, 700);
    assert_eq!(r(&s, B).h, 700);
    assert_eq!(r(&s, X).y, r(&s, B).y + 700);
}

#[test]
fn min_height_is_capped_to_viewport() {
    let mut s = strip();
    s.add_window(A);
    s.set_min_height(A, 5000);
    assert_eq!(r(&s, A).h, 1080);
}

#[test]
fn focus_follows_screen_geometry() {
    let mut s = y_right_x_top_left();
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Left); // kiri: B (atas), X (bawah). kanan: A
    assert_eq!(s.focused(), Some(B));
    s.focus(Dir::Down);
    assert_eq!(s.focused(), Some(X));
    s.focus(Dir::Up);
    assert_eq!(s.focused(), Some(B));
    s.focus(Dir::Right);
    assert_eq!(s.focused(), Some(A));
    s.focus(Dir::Left);
    assert_eq!(s.focused(), Some(B));
}

#[test]
fn remove_moves_focus_and_clamps_scroll() {
    let mut s = strip();
    s.add_window(A);
    s.add_window(B);
    s.add_window(C);
    s.remove_window(C);
    assert_eq!(s.items().len(), 2);
    assert_eq!(s.focused(), Some(B));
    assert_eq!(r(&s, B).y, 0);
}

#[test]
fn vertical_focus_scrolls_back_up() {
    let mut s = strip();
    s.add_window(A);
    s.add_window(B);
    s.focus(Dir::Up);
    assert_eq!(s.focused(), Some(A));
    assert_eq!(r(&s, A).y, 0);
}

#[test]
fn full_width_goes_below_window_beside_it() {
    // A kiri atas, B kanan atas. A lebar penuh -> turun di bawah B.
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Right);
    s.set_focus(A);
    s.snap(Dir::Right);
    assert_eq!(r(&s, A), rect(0, 540, 1920, 540));
    assert_eq!(r(&s, B), rect(960, 0, 960, 540));
}

#[test]
fn goes_below_when_quarter_row_is_complete() {
    // A kiri atas, B kanan atas (baris kuarter penuh). C kuarter kiri -> di bawah A.
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Right);
    s.add_window(C);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    assert_eq!(r(&s, A), rect(0, 0, 960, 540));
    assert_eq!(r(&s, B), rect(960, 0, 960, 540));
    assert_eq!(r(&s, C), rect(0, 540, 960, 540));
}

#[test]
fn maximize_goes_below_neighbors_without_dragging_them() {
    // kiri: B (atas), A (bawah). kanan: C setengah layar penuh tinggi.
    let mut s = strip();
    s.add_window(C);
    s.snap(Dir::Right);
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Left); // B di atas A
    s.set_focus(A);
    s.maximize();
    assert_eq!(r(&s, A).w, 1920);
    assert_eq!(r(&s, A).h, 1080);
    assert_eq!(r(&s, A).y - r(&s, C).y, 1080); // di bawah A dan C
    assert_eq!(r(&s, C).y, r(&s, B).y); // C tidak ikut turun
}

#[test]
fn move_window_with_full_width_neighbor_keeps_sizes() {
    // A penuh lebar setengah tinggi; di bawahnya B kiri, C kanan.
    // B naik: B tetap kuarter, A tetap penuh lebar, C ikut turun di bawah A.
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(C);
    s.snap(Dir::Up);
    s.snap(Dir::Right);
    s.set_focus(B);
    s.move_window(Dir::Up);
    assert_eq!(s.focused(), Some(B));
    assert_eq!(r(&s, B), rect(0, 0, 960, 540));
    assert_eq!(r(&s, A), rect(0, 540, 1920, 540));
    assert_eq!(r(&s, C).y, 1080);
}

#[test]
fn move_window_in_same_lane_swaps_positions_not_sizes() {
    // kiri: W (tinggi penuh) di atas, X (setengah) di bawah. Kanan: Y.
    let mut s = strip();
    s.add_window(A); // A = Y
    s.snap(Dir::Right);
    s.add_window(X);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(B); // B = W
    s.snap(Dir::Left); // di atas X, tinggi penuh
    s.set_focus(X);
    let (wh, xh) = (r(&s, B).h, r(&s, X).h);
    assert_eq!((wh, xh), (1080, 540));
    s.move_window(Dir::Up);
    assert_eq!(r(&s, X).h, 540);
    assert_eq!(r(&s, B).h, 1080);
    assert_eq!(r(&s, B).y - r(&s, X).y, 540); // X sekarang di atas B
    assert_eq!(r(&s, X).y, r(&s, A).y);
}

#[test]
fn move_window_across_lanes_swaps_sides_not_heights() {
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Up);
    s.snap(Dir::Right); // B sepertiga di kanan
    s.set_focus(A);
    s.move_window(Dir::Right);
    assert_eq!(r(&s, A), rect(960, 0, 960, 540));
    assert_eq!(r(&s, B), rect(0, 0, 960, 360));
}

#[test]
fn changing_height_forgets_measured_min_height() {
    let mut s = strip();
    s.add_window(A);
    s.set_min_height(A, 600);
    assert_eq!(r(&s, A).h, 1080);
    s.snap(Dir::Up);
    assert_eq!(r(&s, A).h, 540); // minimum lama dilupakan, diukur ulang
}

#[test]
fn gap_around_single_window() {
    let mut s = strip();
    s.set_gap(20);
    s.add_window(A);
    assert_eq!(r(&s, A), rect(20, 20, 1880, 1040));
}

#[test]
fn gap_between_lanes_equals_gap_setting() {
    let mut s = strip();
    s.set_gap(20);
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Right);
    let (a, b) = (r(&s, A), r(&s, B));
    assert_eq!(b.x - (a.x + a.w), 20);
    assert_eq!(a.x, 20); // jarak ke tepi = gap
    assert_eq!(1920 - (b.x + b.w), 20);
}

#[test]
fn gap_between_rows_equals_gap_setting() {
    let mut s = strip();
    s.set_gap(20);
    s.add_window(A);
    s.snap(Dir::Up);
    s.add_window(B);
    s.snap(Dir::Up);
    let (a, b) = (r(&s, A), r(&s, B));
    assert_eq!(b.y - (a.y + a.h), 20);
}

#[test]
fn min_height_accounts_for_gap() {
    let mut s = strip();
    s.set_gap(20);
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Up);
    s.set_min_height(A, 600);
    assert_eq!(r(&s, A).h, 600);
}

#[test]
fn min_height_mismatch_is_evened_out_before_full_window() {
    // A kiri setengah, B kanan setengah, C penuh di bawah. B punya tinggi
    // minimum lebih besar -> A diregangkan supaya dasar A dan B sejajar.
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Right);
    s.add_window(C);
    s.set_min_height(B, 600);
    assert_eq!(r(&s, B).h, 600);
    assert_eq!(r(&s, A).h, 600);
    assert_eq!(r(&s, C).y - r(&s, B).y, 600);
}

#[test]
fn intentionally_unaligned_lanes_are_not_stretched() {
    // A setengah di kiri, B sepertiga di kanan: memang tidak sejajar.
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Up);
    s.snap(Dir::Right);
    s.add_window(C);
    assert_eq!(r(&s, A).h, 540);
    assert_eq!(r(&s, B).h, 360);
}

#[test]
fn empty_lane_is_not_filled() {
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(C);
    assert_eq!(r(&s, A).h, 540);
    assert_eq!(r(&s, C).y - r(&s, A).y, 540);
}

#[test]
fn hidden_window_frees_its_space_and_returns_to_same_slot() {
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Right);
    assert_eq!(r(&s, B), rect(960, 0, 960, 540));
    assert!(s.set_hidden(B, true));
    assert_eq!(s.layout().len(), 1);
    assert_eq!(s.focused(), Some(A));
    assert!(s.set_hidden(B, false));
    assert_eq!(r(&s, B), rect(960, 0, 960, 540));
    assert!(!s.set_hidden(B, false)); // sudah terlihat
}

#[test]
fn hiding_everything_then_showing_restores_the_whole_layout() {
    // 2x2 + satu window penuh di bawah, lalu "Win+D" dan buka lagi
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Right);
    s.add_window(C);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(D);
    s.snap(Dir::Up);
    s.snap(Dir::Right);
    s.add_window(X);
    let ids = [A, B, C, D, X];
    let shape = |s: &Strip| -> Vec<(i32, i32, i32, i32)> {
        let ay = r(s, A).y;
        ids.iter()
            .map(|&i| {
                let q = r(s, i);
                (q.x, q.y - ay, q.w, q.h)
            })
            .collect()
    };
    let before = shape(&s);
    for &i in &ids {
        s.set_hidden(i, true);
    }
    assert!(s.layout().is_empty());
    assert_eq!(s.focused(), None);
    for &i in ids.iter().rev() {
        s.set_hidden(i, false); // urutan muncul tidak harus sama
    }
    assert_eq!(shape(&s), before);
    assert!(s.focused().is_some());
}

#[test]
fn hidden_window_is_skipped_by_focus_and_removal() {
    let mut s = strip();
    s.add_window(A);
    s.add_window(B);
    s.add_window(C);
    s.set_hidden(B, true);
    s.remove_window(C);
    assert_eq!(s.focused(), Some(A)); // B tersembunyi, tidak dipilih
    s.focus(Dir::Down);
    assert_eq!(s.focused(), Some(A)); // tidak ada tetangga terlihat
    s.set_focus(B); // diabaikan selama tersembunyi
    assert_eq!(s.focused(), Some(A));
}

#[test]
fn overview_describes_whole_strip_including_offscreen_part() {
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Right);
    s.add_window(C);
    s.snap(Dir::Up);
    let ov = s.overview();
    let at = |id| ov.items.iter().find(|i| i.id == id).unwrap().rect;
    assert_eq!((ov.width, ov.height), (1920, 1080));
    assert_eq!(at(A), rect(0, 0, 960, 540));
    assert_eq!(at(B), rect(960, 0, 960, 540));
    assert_eq!(at(C), rect(0, 540, 1920, 540));

    s.add_window(D); // satu layar penuh lagi di bawah
    let ov = s.overview();
    assert_eq!(ov.height, 2160);
    assert_eq!(ov.view_y, 1080); // yang tampil: D saja
    assert_eq!(ov.view_h, 1080);
}

#[test]
fn overview_skips_hidden_windows() {
    let mut s = strip();
    s.add_window(A);
    s.add_window(B);
    s.set_hidden(B, true);
    assert_eq!(s.overview().items.len(), 1);
}

#[test]
fn neighbor_of_does_not_change_focus() {
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Right);
    assert_eq!(s.focused(), Some(B));
    assert_eq!(s.neighbor_of(A, Dir::Right), Some(B));
    assert_eq!(s.neighbor_of(B, Dir::Left), Some(A));
    assert_eq!(s.neighbor_of(A, Dir::Up), None);
    assert_eq!(s.focused(), Some(B));
}

#[test]
fn cycle_follows_reading_order_and_wraps() {
    // A kiri atas, B kanan atas, C penuh di bawah
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Right);
    s.add_window(C);
    s.snap(Dir::Up);
    assert_eq!(s.cycle_from(A, true), Some(B));
    assert_eq!(s.cycle_from(B, true), Some(C));
    assert_eq!(s.cycle_from(C, true), Some(A)); // berputar
    assert_eq!(s.cycle_from(A, false), Some(C));
    assert_eq!(s.cycle_from(C, false), Some(B));
    assert_eq!(s.focused(), Some(C)); // fokus tidak berubah
}

#[test]
fn cycle_skips_hidden_and_needs_two_windows() {
    let mut s = strip();
    s.add_window(A);
    assert_eq!(s.cycle_from(A, true), None);
    s.add_window(B);
    s.add_window(C);
    s.set_hidden(B, true);
    assert_eq!(s.cycle_from(A, true), Some(C));
    assert_eq!(s.cycle_from(B, true), None); // tersembunyi
}

/// [A(kiri), B(penuh), C(kiri)]: dua blok dipisah B.
fn blocks() -> Strip {
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Left);
    s.add_window(B);
    s.add_window(C);
    s.snap(Dir::Left);
    s
}

fn order(s: &Strip) -> Vec<(WindowId, Span)> {
    s.items().iter().map(|i| (i.id, i.span)).collect()
}

#[test]
fn insert_beside_takes_the_opposite_lane_of_that_block() {
    let mut s = blocks();
    assert_eq!(
        order(&s),
        vec![(A, Span::Left), (B, Span::Full), (C, Span::Left)]
    );
    assert!(s.insert_window(D, InsertPosition::Beside(A)));
    assert_eq!(
        order(&s),
        vec![(A, Span::Left), (D, Span::Right), (B, Span::Full), (C, Span::Left)]
    );
    assert_eq!(s.focused(), Some(D));

    // blok bawah: di sebelah C
    let mut s = blocks();
    assert!(s.insert_window(D, InsertPosition::Beside(C)));
    assert_eq!(
        order(&s),
        vec![(A, Span::Left), (B, Span::Full), (C, Span::Left), (D, Span::Right)]
    );
}

#[test]
fn insert_beside_matches_the_row_when_the_other_lane_has_windows() {
    // kiri: A, A2; kanan: E. Di sebelah A2 -> urutan ke-2 di lajur kanan (di bawah E)
    let make_strip = || {
        let mut s = strip();
        s.add_window(A);
        s.snap(Dir::Left);
        assert!(s.insert_window(B, InsertPosition::Split(A, false)));
        assert!(s.insert_window(C, InsertPosition::Beside(A)));
        s
    };
    let mut s = make_strip();
    assert!(s.insert_window(D, InsertPosition::Beside(B)));
    let right: Vec<WindowId> = s
        .items()
        .iter()
        .filter(|i| i.span == Span::Right)
        .map(|i| i.id)
        .collect();
    assert_eq!(right, vec![C, D]);
    // di sebelah A -> urutan pertama, di atas C
    let mut s2 = make_strip();
    assert!(s2.insert_window(D, InsertPosition::Beside(A)));
    let right: Vec<WindowId> = s2
        .items()
        .iter()
        .filter(|i| i.span == Span::Right)
        .map(|i| i.id)
        .collect();
    assert_eq!(right, vec![D, C]);
}

#[test]
fn insert_full_goes_to_block_boundaries_without_splitting_blocks() {
    // di atas blok pertama / di bawah blok terakhir
    let mut s = blocks();
    assert!(s.insert_window(D, InsertPosition::FullBefore(A)));
    assert_eq!(s.items()[0].id, D);
    assert_eq!(s.items()[0].span, Span::Full);
    let mut s = blocks();
    assert!(s.insert_window(D, InsertPosition::FullAfter(C)));
    assert_eq!(s.items()[3].id, D);
    assert_eq!(s.items()[3].span, Span::Full);
    // di bawah blok A (window setengah lebar): sebelum B, bukan di tengah blok
    let mut s = blocks();
    assert!(s.insert_window(D, InsertPosition::FullAfter(A)));
    assert_eq!(
        order(&s),
        vec![(A, Span::Left), (D, Span::Full), (B, Span::Full), (C, Span::Left)]
    );
    // relatif terhadap window penuh lebar
    let mut s = blocks();
    assert!(s.insert_window(D, InsertPosition::FullBefore(B)));
    assert_eq!(s.items()[1].id, D);
    let mut s = blocks();
    assert!(s.insert_window(D, InsertPosition::FullAfter(B)));
    assert_eq!(s.items()[2].id, D);
}

#[test]
fn split_halves_a_full_window_then_stacks_in_the_lane() {
    // satu window penuh: drop di atasnya -> berdampingan, masing-masing setengah lebar
    let mut s = strip();
    s.add_window(A);
    assert!(s.insert_window(B, InsertPosition::Split(A, false)));
    assert_eq!(order(&s), vec![(A, Span::Left), (B, Span::Right)]);
    assert_eq!(s.items()[0].height, Height::Full);
    assert_eq!(s.items()[1].height, Height::Full);

    // drop di sisi kanan (B): B dan window baru bertumpuk, masing-masing setengah tinggi
    assert!(s.insert_window(C, InsertPosition::Split(B, false)));
    assert_eq!(
        order(&s),
        vec![(A, Span::Left), (B, Span::Right), (C, Span::Right)]
    );
    assert_eq!(s.items()[1].height, Height::Half);
    assert_eq!(s.items()[2].height, Height::Half);

    // drop di A: empat window sama besar
    assert!(s.insert_window(D, InsertPosition::Split(A, false)));
    assert_eq!(
        order(&s),
        vec![
            (A, Span::Left),
            (D, Span::Left),
            (B, Span::Right),
            (C, Span::Right)
        ]
    );
    assert!(s.items().iter().all(|i| i.height == Height::Half));
}

#[test]
fn split_first_puts_the_new_window_left_or_above() {
    let mut s = strip();
    s.add_window(A);
    assert!(s.insert_window(B, InsertPosition::Split(A, true)));
    assert_eq!(order(&s), vec![(A, Span::Right), (B, Span::Left)]);

    // di separuh atas window setengah lebar: window baru di atasnya
    let mut s = strip();
    s.add_window(A);
    assert!(s.insert_window(B, InsertPosition::Split(A, false))); // A kiri, B kanan
    assert!(s.insert_window(C, InsertPosition::Split(B, true)));
    let right: Vec<WindowId> = s
        .items()
        .iter()
        .filter(|i| i.span == Span::Right)
        .map(|i| i.id)
        .collect();
    assert_eq!(right, vec![C, B]); // C di atas B
}

#[test]
fn halving_goes_below_half_for_nested_splits() {
    let mut s = strip();
    s.add_window(A);
    assert!(s.insert_window(B, InsertPosition::Split(A, false)));
    assert!(s.insert_window(C, InsertPosition::Split(B, false))); // B, C: 1/2
    assert!(s.insert_window(D, InsertPosition::Split(C, false))); // C, D: 1/4
    let h = |id| s.items().iter().find(|i| i.id == id).unwrap().height;
    assert_eq!(h(B), Height::Half);
    assert_eq!(h(C), Height::Custom(250));
    assert_eq!(h(D), Height::Custom(250));
}

#[test]
fn split_rejects_unknown_and_hidden_targets() {
    let mut s = strip();
    s.add_window(A);
    assert!(!s.insert_window(B, InsertPosition::Split(WindowId(99), false)));
    assert!(!s.insert_window(A, InsertPosition::Split(A, false))); // sudah ada
    s.add_window(C);
    s.set_hidden(C, true);
    assert!(!s.insert_window(B, InsertPosition::Split(C, false)));
    assert_eq!(s.items().len(), 2);
}

#[test]
fn insert_rejects_invalid_requests() {
    let mut s = blocks();
    assert!(!s.insert_window(D, InsertPosition::Beside(B))); // B penuh lebar
    assert!(!s.insert_window(D, InsertPosition::Beside(WindowId(99)))); // tidak dikenal
    assert!(!s.insert_window(A, InsertPosition::FullAfter(C))); // sudah ada
    assert_eq!(s.items().len(), 3);
}

#[test]
fn swap_windows_exchanges_order_or_sides() {
    // satu lajur penuh: tukar urutan
    let mut s = strip();
    s.add_window(A);
    s.add_window(B);
    s.add_window(C);
    assert!(s.swap_windows(A, C));
    let order: Vec<WindowId> = s.items().iter().map(|i| i.id).collect();
    assert_eq!(order, vec![C, B, A]);
    assert!(!s.swap_windows(A, A));
    assert!(!s.swap_windows(A, D)); // tidak dikenal

    // beda lajur: tukar sisi, urutan tetap
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Left);
    s.add_window(B);
    s.snap(Dir::Right);
    assert!(s.swap_windows(A, B));
    assert_eq!(s.items()[0].id, A);
    assert_eq!(s.items()[0].span, Span::Right);
    assert_eq!(s.items()[1].span, Span::Left);

    // window tersembunyi tidak ikut
    s.set_hidden(B, true);
    assert!(!s.swap_windows(A, B));
}

#[test]
fn wrap_neighbor_jumps_to_the_opposite_end() {
    // A kiri atas, B kanan atas, C penuh di bawah
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Right);
    s.add_window(C);
    s.snap(Dir::Up);
    // Atas di baris teratas -> baris terbawah
    assert_eq!(s.wrap_neighbor_of(A, Dir::Up), Some(C));
    assert_eq!(s.wrap_neighbor_of(B, Dir::Up), Some(C));
    // Bawah di baris terbawah -> baris teratas (A atau B, sama-sama menutup lebar C)
    assert!(matches!(s.wrap_neighbor_of(C, Dir::Down), Some(w) if w == A || w == B));
    // Kiri di tepi kiri -> lajur kanan, baris yang sama; sebaliknya untuk Kanan
    assert_eq!(s.wrap_neighbor_of(A, Dir::Left), Some(B));
    assert_eq!(s.wrap_neighbor_of(B, Dir::Right), Some(A));
    // window penuh lebar tidak punya "sisi seberang" di arah horizontal
    assert_eq!(s.wrap_neighbor_of(C, Dir::Left), None);
    assert_eq!(s.wrap_neighbor_of(C, Dir::Right), None);
    assert_eq!(s.focused(), Some(C)); // fokus tidak berubah
}

#[test]
fn wrap_neighbor_in_a_single_column_matches_reading_order() {
    let mut s = strip();
    s.add_window(A);
    s.add_window(B);
    s.add_window(C); // tiga window penuh bertumpuk
    assert_eq!(s.wrap_neighbor_of(A, Dir::Up), Some(C));
    assert_eq!(s.wrap_neighbor_of(C, Dir::Down), Some(A));
    assert_eq!(s.wrap_neighbor_of(B, Dir::Left), None);
    assert_eq!(s.wrap_neighbor_of(B, Dir::Right), None);
}

#[test]
fn wrap_neighbor_needs_two_visible_windows() {
    let mut s = strip();
    s.add_window(A);
    for d in [Dir::Up, Dir::Down, Dir::Left, Dir::Right] {
        assert_eq!(s.wrap_neighbor_of(A, d), None);
    }
    s.add_window(B);
    s.set_hidden(B, true);
    assert_eq!(s.wrap_neighbor_of(A, Dir::Up), None); // B tersembunyi
    assert_eq!(s.wrap_neighbor_of(B, Dir::Up), None); // id tersembunyi
    assert_eq!(s.wrap_neighbor_of(D, Dir::Up), None); // id tidak dikenal
}

fn two_half_lanes() -> Strip {
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Right);
    s
}

#[test]
fn adjust_split_moves_the_lane_divider() {
    let mut s = two_half_lanes();
    s.set_focus(A);
    assert!(s.adjust_split(96));
    assert_eq!(r(&s, A), rect(0, 0, 1056, 540));
    assert_eq!(r(&s, B), rect(1056, 0, 864, 540));
    s.set_focus(B);
    assert!(s.adjust_split(-192)); // dari 1056 ke 864
    assert_eq!(r(&s, A).w, 864);
    assert_eq!(r(&s, B), rect(864, 0, 1056, 540));
}

#[test]
fn divider_stops_at_the_global_limits_without_piling_up() {
    let mut s = two_half_lanes();
    s.set_focus(A);
    assert!(s.adjust_split(500));
    assert!(s.adjust_split(500));
    assert_eq!(r(&s, A).w, 1632); // 85%
    assert!(!s.adjust_split(500)); // sudah mentok
    assert!(s.adjust_split(-96)); // dan langsung bisa balik
    assert_eq!(r(&s, A).w, 1536);
}

#[test]
fn divider_belongs_to_the_lane_so_stacked_windows_follow() {
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Right);
    s.add_window(C);
    s.snap(Dir::Up);
    s.snap(Dir::Left); // C di bawah A, lajur kiri
    s.set_focus(A);
    assert!(s.adjust_split(96));
    assert_eq!(r(&s, A).w, 1056);
    assert_eq!(r(&s, C).w, 1056);
    assert_eq!(r(&s, B).w, 864);
}

#[test]
fn divider_is_stored_per_block() {
    // blok 1: A B. Window penuh C. Blok 2: D X.
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(B);
    s.snap(Dir::Up);
    s.snap(Dir::Right);
    s.add_window(C);
    s.snap(Dir::Up);
    s.add_window(D);
    s.snap(Dir::Up);
    s.snap(Dir::Left);
    s.add_window(X);
    s.snap(Dir::Up);
    s.snap(Dir::Right);
    s.set_focus(A);
    assert!(s.adjust_split(96));
    assert_eq!(r(&s, A).w, 1056);
    assert_eq!((r(&s, D).w, r(&s, X).w), (960, 960)); // blok 2 tidak ikut
    s.set_focus(D);
    assert!(s.adjust_split(-96));
    assert_eq!(r(&s, D).w, 864);
    assert_eq!(r(&s, A).w, 1056); // blok 1 tidak ikut
    assert_eq!(r(&s, C).w, 1920); // window penuh tidak punya pembagi
}

#[test]
fn full_width_window_has_no_divider() {
    let mut s = strip();
    s.add_window(A);
    assert!(!s.adjust_split(96));
    assert!(!s.reset_split());
}

#[test]
fn reset_returns_to_even_split() {
    let mut s = two_half_lanes();
    s.set_focus(A);
    s.adjust_split(300);
    assert!(r(&s, A).w > 960);
    assert!(s.reset_split());
    assert_eq!(r(&s, A).w, 960);
    assert!(!s.reset_split()); // sudah 50/50
}

#[test]
fn app_minimum_width_pins_the_divider() {
    let mut s = two_half_lanes();
    s.set_focus(A);
    assert!(s.set_min_width(B, 1000));
    assert_eq!((r(&s, A).w, r(&s, B).w, r(&s, B).x), (920, 1000, 920));
    assert!(s.adjust_split(100)); // lajur kanan diukur ulang
    assert!(r(&s, B).w < 1000);
    assert!(s.set_min_width(B, 1000)); // pengukuran ulang: app tetap menolak
    assert_eq!(r(&s, A).w, 920);
    assert!(!s.adjust_split(100)); // tidak bergeser dan tidak menumpuk
}

#[test]
fn min_width_is_capped_to_the_content_width() {
    let mut s = two_half_lanes();
    assert!(s.set_min_width(B, 99_999));
    assert!(r(&s, B).w <= 1920);
}

#[test]
fn divider_shows_up_in_overview_and_neighbor_lookup() {
    let mut s = two_half_lanes();
    s.set_focus(A);
    s.adjust_split(96);
    let ov = s.overview();
    let at = |id| ov.items.iter().find(|i| i.id == id).unwrap().rect;
    assert_eq!(at(A), rect(0, 0, 1056, 540));
    assert_eq!(at(B), rect(1056, 0, 864, 540));
    assert_eq!(s.neighbor_of(A, Dir::Right), Some(B));
    assert_eq!(s.neighbor_of(B, Dir::Left), Some(A));
}

#[test]
fn gap_between_unequal_lanes_stays_constant() {
    let mut s = two_half_lanes();
    s.set_gap(20);
    s.set_focus(A);
    s.adjust_split(96);
    let (a, b) = (r(&s, A), r(&s, B));
    assert_eq!(b.x - (a.x + a.w), 20);
    assert_eq!(a.x, 20);
    assert_eq!(1920 - (b.x + b.w), 20);
}

#[test]
fn step_height_moves_by_pixels() {
    let mut s = strip();
    assert!(!s.adjust_height(-80)); // belum ada window
    s.add_window(A);
    assert!(s.adjust_height(-100));
    assert!((r(&s, A).h - 980).abs() <= 1);
    assert_eq!(r(&s, A).w, 1920); // tetap penuh lebar
}

#[test]
fn step_height_magnets_to_presets() {
    let mut s = strip();
    s.add_window(A);
    assert!(s.adjust_height(-500)); // 1080 -> 580, dekat 1/2
    assert_eq!(s.items()[0].height, Height::Half);
    // turun per 80 px dari penuh akhirnya mendarat tepat di 1/2
    let mut s = strip();
    s.add_window(A);
    let mut n = 0;
    while s.items()[0].height != Height::Half && n < 20 {
        s.adjust_height(-80);
        n += 1;
    }
    assert_eq!(s.items()[0].height, Height::Half);
    assert_eq!(r(&s, A).h, 540);
}

#[test]
fn step_height_always_leaves_the_current_preset() {
    for step in [10, 80, 400] {
        for ups in 0..3 {
            let mut s = strip();
            s.add_window(A);
            for _ in 0..ups {
                s.snap(Dir::Up);
            }
            let before = s.items()[0].height;
            assert!(s.adjust_height(-step), "step {step}, ups {ups}");
            assert_ne!(s.items()[0].height, before);
        }
    }
}

#[test]
fn step_height_is_clamped_to_15_percent_and_full() {
    let mut s = strip();
    s.add_window(A);
    for _ in 0..30 {
        s.adjust_height(-300);
    }
    assert_eq!(r(&s, A).h, 162);
    assert!(!s.adjust_height(-300));
    assert!(s.adjust_height(10_000));
    assert_eq!(s.items()[0].height, Height::Full);
    assert!(!s.adjust_height(100));
}

#[test]
fn step_height_does_not_pile_up_against_app_minimum() {
    let mut s = strip();
    s.add_window(A);
    s.snap(Dir::Up); // 1/2 = 540
    s.set_min_height(A, 600);
    assert_eq!(r(&s, A).h, 600);
    for _ in 0..5 {
        s.adjust_height(-80); // mulai dari 600 (sebenarnya), bukan 540
        s.set_min_height(A, 600); // app tetap menolak di bawah 600
        assert_eq!(r(&s, A).h, 600);
    }
    assert!(matches!(
        s.items()[0].height,
        Height::Half | Height::Custom(481)
    ));
}

#[test]
fn custom_height_snaps_to_next_preset_and_scales_with_viewport() {
    let mut s = strip();
    s.add_window(A);
    s.adjust_height(-80); // Custom(925)
    s.set_viewport(rect(0, 0, 1920, 540));
    assert!((r(&s, A).h - 500).abs() <= 1); // proporsional, bukan piksel tetap
    s.set_viewport(rect(0, 0, 1920, 1080));
    s.snap(Dir::Up);
    assert_eq!(s.items()[0].height, Height::Half);

    let mut s = strip();
    s.add_window(A);
    s.adjust_height(-80);
    s.snap(Dir::Down);
    assert_eq!(s.items()[0].height, Height::Full);
}
