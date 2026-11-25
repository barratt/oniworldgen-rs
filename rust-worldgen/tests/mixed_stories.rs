//! Run with ONI_STREAMING_ASSETS set to a local build-744825 game install:
//! cargo test -p rust-worldgen --release --test mixed_stories -- --ignored
use oni_worldgen::{compact::{self, Tier}, custom_settings, data_source::DataSource, pipeline, story_traits};
use serde_json::to_value;

#[test]
#[ignore = "requires the installed game's StreamingAssets (ONI_STREAMING_ASSETS)"]
fn mixed_worlds_keep_stories_across_summary_and_full_generation() {
    let assets = std::env::var("ONI_STREAMING_ASSETS").expect("set ONI_STREAMING_ASSETS");
    let ds = DataSource::load(&assets).unwrap();
    for mixing in ["J3ET5", "1UON93X2"] {
        let coordinate = format!("V-SNDST-C-5082-0-D3-{mixing}");
        let checked = pipeline::validate(&coordinate, &ds, None, true).unwrap();
        assert_eq!(checked.coordinate(false, "").unwrap(), coordinate);
        let summary = compact::run(&coordinate, &ds, None, Tier::Summary).unwrap();
        let full = pipeline::run_cluster(&coordinate, &ds, None).unwrap();
        let settings = custom_settings::decode("0", "D3", mixing).unwrap();
        let mut expected = story_traits::chosen(&ds, ds.cluster_for_coord(&coordinate).unwrap(), &settings.stories).unwrap();
        let mut placed: Vec<_> = summary.worlds.iter().flat_map(|w| w.story_traits.clone()).collect();
        placed.sort();
        expected.sort();
        assert!(!expected.is_empty());
        assert_eq!(placed, expected, "each selected story must be placed exactly once");
        assert!(summary.cluster.worlds.iter().any(|w| !w.mixing.subworld_mixing.is_empty()));
        if mixing == "1UON93X2" {
            assert!(!summary.cluster.substitutions.is_empty(), "world mixing must replace a world");
        }
        assert_eq!(summary.worlds.len(), full.worlds.len());
        assert_eq!(summary.cluster.mixing, full.cluster.mixing);
        assert_eq!(to_value(&summary.starmap).unwrap(), to_value(&full.starmap).unwrap());
        for (a, b) in summary.worlds.iter().zip(&full.worlds) {
            assert_eq!(a.story_traits, b.story_traits);
            assert_eq!(to_value(&a.template_placement).unwrap(), to_value(&b.template_placement).unwrap());
            assert_eq!(to_value(&a.runtime_geysers).unwrap(), to_value(&b.runtime_geysers).unwrap());
        }
        // Stories run after layout; keeping mixing but disabling stories must
        // leave the mixed worlds and their biome cells intact.
        let no_stories = compact::run(&format!("V-SNDST-C-5082-0-0-{mixing}"), &ds, None, Tier::Summary).unwrap();
        assert!(no_stories.worlds.iter().all(|w| w.story_traits.is_empty()));
        for (a, b) in summary.cluster.worlds.iter().zip(&no_stories.cluster.worlds) {
            assert_eq!(a.name, b.name);
            assert_eq!(to_value(&a.mixing).unwrap(), to_value(&b.mixing).unwrap());
            assert_eq!(to_value(&a.cells).unwrap(), to_value(&b.cells).unwrap());
        }
    }
}
