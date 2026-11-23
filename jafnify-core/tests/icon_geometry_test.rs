//! The core mark geometry: the SVG parse and the tray arc plan.

use jafnify_core::icon;

#[test]
fn the_mark_geometry_comes_from_the_svg() {
    let mark = icon::mark();
    assert_eq!(mark.arcs.len(), 2, "the J plus two arcs");
    assert!((mark.body.width - 7.0).abs() < 1e-9, "the J stroke width");
    for (i, arc) in mark.arcs.iter().enumerate() {
        assert!((arc.width - 5.5).abs() < 1e-9, "arc {i} stroke width");
        let geom = arc.arc.expect("the arc keeps its geometry");
        assert!(
            (geom.centre.0 - 40.0).abs() < 0.01 && (geom.centre.1 - 16.0).abs() < 0.01,
            "arc {i} is centred on the J top end"
        );
    }
    assert!((mark.arcs[0].arc.unwrap().radius - 10.0).abs() < 0.01);
    assert!((mark.arcs[1].arc.unwrap().radius - 17.0).abs() < 0.01);
}

#[test]
fn the_small_tray_icon_keeps_the_arcs_apart() {
    let large = icon::arc_plan(32);
    assert_eq!(large.len(), 2, "two arcs at 32 px");
    let small = icon::arc_plan(16);
    assert_eq!(small.len(), 1, "the 16 px rule collapses to one arc");
    assert!((small[0].0 - 6.0).abs() < 1e-9, "the small arc is thicker");
    let middle = icon::arc_plan(22);
    assert_eq!(middle.len(), 1, "22 px uses the small variant too");
}
