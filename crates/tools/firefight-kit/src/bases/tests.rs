//! Invented rows only.
use super::*;

fn start(x: f32, y: f32, team: i16) -> StartRow {
    StartRow {
        position: [x, y, 0.],
        team,
    }
}

fn stand(x: f32, y: f32, team: u16) -> FlagRow {
    FlagRow {
        position: [x, y, 1.],
        kind: CTF_FLAG_SPAWN,
        team,
    }
}

#[test]
fn rows_decode_their_position_type_and_team() {
    let mut row = vec![0u8; 0x34];
    row[..4].copy_from_slice(&3.5f32.to_le_bytes());
    row[8..12].copy_from_slice(&(-2f32).to_le_bytes());
    row[START_TEAM..START_TEAM + 2].copy_from_slice(&(-1i16).to_le_bytes());
    assert_eq!(start_row(&row), start(3.5, 0., -1).with_z(-2.));
    let mut row = vec![0u8; NETGAME_FLAG];
    row[4..8].copy_from_slice(&7f32.to_le_bytes());
    row[0x10..0x12].copy_from_slice(&11u16.to_le_bytes());
    row[0x12..0x14].copy_from_slice(&1u16.to_le_bytes());
    assert_eq!(
        flag_row(&row),
        FlagRow {
            position: [0., 7., 0.],
            kind: 11,
            team: 1
        }
    );
}

impl StartRow {
    fn with_z(mut self, z: f32) -> Self {
        self.position[2] = z;
        self
    }
}

#[test]
fn home_base_is_read_from_team_starting_points() {
    let starts = [
        start(0., 0., 0),
        start(2., 0., 0),
        start(40., 10., 1),
        start(44., 10., 1),
        start(20., 50., 8),
    ];
    let bases = find(&starts, &[], Team::Red).unwrap();
    assert_eq!(bases.source, Source::TeamStarts);
    assert_eq!(bases.points, [[1., 0., 0.], [42., 10., 0.]]);
    assert_eq!(bases.home, Team::Red);
}

#[test]
fn home_base_prefers_the_ctf_flag_stand() {
    let starts = [start(0., 0., 0), start(40., 0., 1)];
    let mut other = stand(500., 500., 0);
    other.kind = 1; // a flag return point is not a stand
    let flags = [stand(-4., 2., 0), other, stand(48., 2., 1)];
    let bases = find(&starts, &flags, Team::Red).unwrap();
    assert_eq!(bases.source, Source::FlagStands);
    assert_eq!(bases.points, [[-4., 2., 1.], [48., 2., 1.]]);
    // One team's stand alone is not a pair of bases.
    let bases = find(&starts, &flags[..2], Team::Red).unwrap();
    assert_eq!(bases.source, Source::TeamStarts);
}

#[test]
fn the_home_option_swaps_the_bases() {
    let flags = [stand(-4., 2., 0), stand(48., 2., 1)];
    let red = find(&[], &flags, Team::Red).unwrap();
    let blue = find(&[], &flags, Team::Blue).unwrap();
    assert_eq!(blue.points, [red.points[1], red.points[0]]);
    assert_eq!(blue.home, Team::Blue);
    assert_eq!(Team::parse("BLUE"), Some(Team::Blue));
    assert_eq!(Team::parse("red"), Some(Team::Red));
    assert_eq!(Team::parse("green"), None);
    assert_eq!(Config::default().home, Team::Red);
}

#[test]
fn bases_split_untagged_starts_into_two_clusters() {
    let starts = [
        start(1., 0., 8),
        start(30., 1., 8),
        start(0., 2., 8),
        start(31., -1., 8),
        start(2., 1., 8),
        start(29., 0., 8),
    ];
    let bases = find(&starts, &[], Team::Red).unwrap();
    assert_eq!(bases.source, Source::StartClusters);
    // The farthest pair is rows 2 and 3; row 2's cluster is team 0's.
    assert_eq!(bases.points, [[1., 1., 0.], [30., 0., 0.]]);
    assert_eq!(find(&starts, &[], Team::Red), find(&starts, &[], Team::Red));
    let swapped = find(&starts, &[], Team::Blue).unwrap();
    assert_eq!(swapped.points, [bases.points[1], bases.points[0]]);
    // No two distinct points, no bases.
    assert_eq!(
        find(&[start(1., 1., 8), start(1., 1.5, 8)], &[], Team::Red),
        None
    );
    assert_eq!(find(&[], &[], Team::Red), None);
}

#[test]
fn starting_locations_take_their_base_team() {
    let bases = [[0., 0., 0.], [40., 0., 0.]];
    let starts = [
        start(1., 0., 8),
        start(39., 5., 0),
        start(20., 0., 1),
        start(25., 0., 3),
    ];
    // The midpoint goes to the home base.
    assert_eq!(start_teams(&bases, &starts), [0, 1, 0, 1]);
}
