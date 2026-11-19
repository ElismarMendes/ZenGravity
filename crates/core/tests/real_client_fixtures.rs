//! Suíte de testes de regressão com fixtures reais do cliente MU Online ("The Classic"):
//! - 3 recortes reais do campo de Zen do inventário (`tests/fixtures/zen_field/`)
//! - 72 recortes reais do número de HP acima da esfera (`tests/fixtures/hp_field/`)
//! - 44 recortes reais do canto superior esquerdo com nome de mapa e coordenadas (`tests/fixtures/coords/`)
//! - 6 recortes reais do ícone Play/Pause do MU Helper (`tests/fixtures/helper/`)
//! - 2 arquivos reais criptografados de terreno (`EncTerrain1.att` Lorencia e `EncTerrain38.att` Kanturu)

use zengravity_core::glyphs::{helper_on, map_is_kanturu, read_coords, read_number, Bits};
use zengravity_core::hp_pixel::read_hp;
use zengravity_core::map::{world_float_to_tile, TerrainGrid, WorldId};

#[test]
fn real_zen_field_fixtures_match_exact_values() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/zen_field");
    let mut count = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let expected: u64 = path.file_stem().unwrap().to_str().unwrap().parse().unwrap();
        let bits = Bits::from_text(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(read_number(&bits), Some(expected), "{}", path.display());
        count += 1;
    }
    assert!(count >= 3);
}

#[test]
fn real_hp_field_fixtures_match_all_72_captures() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/hp_field");
    let mut count = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let stem = path.file_stem().unwrap().to_str().unwrap();
        let expected: u32 = stem.split('_').next().unwrap().parse().unwrap();
        let bits = Bits::from_text(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(read_hp(&bits), Some(expected), "{}", path.display());
        count += 1;
    }
    assert!(count >= 70, "esperados >= 70 recortes, lidos {count}");
}

#[test]
fn real_coordinate_and_kanturu_corner_fixtures_match() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/coords");
    let mut count = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let stem = path.file_stem().unwrap().to_str().unwrap();
        let mut parts = stem.split('_');
        let x: u32 = parts.next().unwrap().parse().unwrap();
        let y: u32 = parts.next().unwrap().parse().unwrap();
        let bits = Bits::from_text(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(read_coords(&bits), Some((x, y)), "{stem}");
        assert!(map_is_kanturu(&bits), "{}", path.display());
        count += 1;
    }
    assert!(count >= 40);
}

#[test]
fn real_helper_icon_fixtures_distinguish_on_and_off() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/helper");
    let mut count = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let filename = path.file_name().unwrap().to_str().unwrap();
        let bits = Bits::from_text(&std::fs::read_to_string(&path).unwrap());
        let expected = filename.starts_with("ligado");
        assert_eq!(helper_on(&bits), Some(expected), "{filename}");
        count += 1;
    }
    assert!(count >= 5);
}

#[test]
fn real_encrypted_client_terrain_files_decrypt_and_find_valid_paths() {
    let lor_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/terrain/EncTerrain1.att"
    );
    let lor = TerrainGrid::from_client_att_bytes(&std::fs::read(lor_path).unwrap()).unwrap();
    assert_eq!(lor.world, WorldId::Lorencia);
    assert!(lor.from_client_att);
    assert_eq!(lor.attr(135, 123), Some(5));

    let kan_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/terrain/EncTerrain38.att"
    );
    let kan = TerrainGrid::from_client_att_bytes(&std::fs::read(kan_path).unwrap()).unwrap();
    assert_eq!(kan.world, WorldId::Kanturu);
    for (x, y) in [(158, 195), (160, 197), (123, 233), (122, 235)] {
        assert!(kan.is_walkable(x, y), "tile ({x},{y}) deve ser andavel");
    }
    let safe = (30, 210);
    assert!(kan.is_safezone(safe.0, safe.1) && kan.is_walkable(safe.0, safe.1));
    let route = kan
        .find_path(safe, (160, 197))
        .expect("rota da safezone ate o spot (160,197)");
    assert!(route.len() > 50);
    assert_eq!(world_float_to_tile(15950.0), 159);
}
