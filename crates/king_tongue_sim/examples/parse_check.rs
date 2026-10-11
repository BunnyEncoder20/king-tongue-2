fn main() {
    let bytes = std::fs::read("assets/levels/test_scene.ron").expect("read");
    match king_tongue_sim::Level::from_bytes(&bytes) {
        Ok(l) => println!(
            "OK: {} terrain, {} enemies, spawn {:?}, bounds {:?} .. {:?}",
            l.static_terrain.len(),
            l.enemy_spawns.len(),
            l.player_spawn,
            l.world_bounds.min(),
            l.world_bounds.max()
        ),
        Err(e) => {
            eprintln!("FAIL: {e}");
            std::process::exit(1);
        }
    }
}
