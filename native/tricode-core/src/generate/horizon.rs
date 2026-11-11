//! The Horizon style: cells of one colour (primary or secondary) joined into diagonal "waterfalls" drawn as
//! single shapes.
//!
//! [`HorizonLayout::from_seed`] runs these steps, each a function below:
//! 1. [`pick_candidates`]: most member cells may join a group, picked at random.
//! 2. [`split_into_lanes`]: connected candidates are split along diagonal lanes into waterfalls.
//! 3. [`cut_at_bands`]: waterfalls are cut at random band boundaries.
//! 4. [`chain_waterfalls`]: waterfalls of opposite directions meeting end to end are joined.
//! 5. Every chain of two or more cells gets a group id.
//!
//! Every random choice comes from one hasher fed in a fixed order, so a seed always gives the same
//! layout (the decode fixtures depend on it).

use crate::grid::points_down;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};

/// Chance, in percent, that a member cell may join a Horizon group.
const HORIZON_GROUP_PERCENT: u64 = 90;

/// Horizon waterfalls may be cut at every this many grid rows (before chaining).
const HORIZON_BAND_ROWS: usize = 3;

/// Chance, as (numerator, denominator), that a waterfall is cut where it
/// crosses a band boundary; otherwise it runs on into the next band.
const HORIZON_BAND_CUT_CHANCE: (u64, u64) = (2, 3);

/// A grid cell as `(row, col)`.
type Cell = (usize, usize);

/// A Horizon lane: its direction and index within that family.
type LaneId = (LaneDirection, isize);

/// A run of cells cut from one lane, and that lane.
type Waterfall = (LaneId, Vec<Cell>);

/// Which way a chain of Horizon waterfalls runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChainKind {
    /// Bottom end to top end: a zigzag running down the image.
    Down,
    /// Bottom to bottom or top to top (an east end to a west end): a zigzag
    /// running sideways.
    Sideways,
}

/// The two families of diagonal lanes Horizon waterfalls run along.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum LaneDirection {
    East,
    West,
}

impl LaneDirection {
    fn opposite(self) -> Self {
        match self {
            Self::East => Self::West,
            Self::West => Self::East,
        }
    }

    /// The lane of this family a cell lies on: a diagonal running down to the east, or one
    /// running down to the west.
    fn lane_of(self, (r, c): Cell) -> isize {
        let row_coef = (r / 2) * 2;
        match self {
            Self::East => {
                let col_coef = if r % 2 == 0 { (c + 1) & !1 } else { c & !1 };
                (r * 2 - row_coef + col_coef) as isize
            }
            Self::West => {
                let col_coef = if r % 2 == 1 { (c + 1) & !1 } else { c & !1 };
                col_coef as isize - (r as isize * 2 - row_coef as isize)
            }
        }
    }

    /// Orders a waterfall's cells from its top end to its bottom end: down the lane means down a
    /// row, or within a row east for east lanes and west for west lanes.
    fn down_key(self, (r, c): Cell) -> (usize, isize) {
        match self {
            Self::East => (r, c as isize),
            Self::West => (r, -(c as isize)),
        }
    }
}

/// The cells sharing an edge with one another.
#[derive(Debug, Clone, Copy)]
struct CellGrid {
    rows: usize,
    cols: usize,
}

impl CellGrid {
    /// The cells sharing an edge with `(row, col)`: left, right, and above or below.
    fn neighbours(self, (row, col): Cell) -> Vec<Cell> {
        let mut nb = Vec::with_capacity(3);
        if col > 0 {
            nb.push((row, col - 1));
        }
        if col + 1 < self.cols {
            nb.push((row, col + 1));
        }
        if points_down(row, col) && row > 0 {
            nb.push((row - 1, col));
        }
        if !points_down(row, col) && row + 1 < self.rows {
            nb.push((row + 1, col));
        }
        nb
    }

    /// Splits `cells` into groups connected through shared edges, in breadth-first order from
    /// each group's first cell in `cells`.
    fn connected_components(self, cells: &[Cell]) -> Vec<Vec<Cell>> {
        let members: HashSet<Cell> = cells.iter().copied().collect();
        let mut visited: HashSet<Cell> = HashSet::new();
        let mut components = Vec::new();
        for &start in cells {
            if !visited.insert(start) {
                continue;
            }
            let mut component = Vec::new();
            let mut queue = VecDeque::from([start]);
            while let Some(cell) = queue.pop_front() {
                component.push(cell);
                for next in self.neighbours(cell) {
                    if members.contains(&next) && visited.insert(next) {
                        queue.push_back(next);
                    }
                }
            }
            components.push(component);
        }
        components
    }
}

/// Per-triangle group assignment for the [`super::ImageStyle::Horizon`] style.
///
/// Foreground (bit=1) triangles are joined into groups (see the module docs). Singletons
/// (groups of exactly one triangle) are excluded and stored as `None`; each other group is drawn
/// as one shape.
#[derive(Debug, Clone)]
pub(crate) struct HorizonLayout {
    /// `rows × cols` grid; `Some(group_id)` for grouped triangles, `None` otherwise.
    pub(crate) group_id: Vec<Vec<Option<usize>>>,
}

impl HorizonLayout {
    /// Build the layout joining the cells flagged in `members` (the primary cells, or the
    /// secondary ones). `seed` drives every random choice.
    pub(crate) fn from_seed(seed: u64, members: &[Vec<bool>]) -> Self {
        let grid = CellGrid {
            rows: members.len(),
            cols: members.first().map_or(0, Vec::len),
        };
        let mut hasher = DefaultHasher::new();
        seed.hash(&mut hasher);
        let candidates = pick_candidates(members, &mut hasher);
        let waterfalls = split_into_lanes(grid, &grid.connected_components(&candidates));
        let waterfalls = cut_at_bands(grid, waterfalls, &mut hasher);
        let chains = chain_waterfalls(grid, waterfalls);

        let mut group_id = vec![vec![None; grid.cols]; grid.rows];
        for (id, members) in chains.iter().filter(|m| m.len() >= 2).enumerate() {
            for &(r, c) in members {
                group_id[r][c] = Some(id);
            }
        }
        Self { group_id }
    }
}

/// The member cells that may join a group, in row-major order: each with
/// [`HORIZON_GROUP_PERCENT`] probability, so most can.
fn pick_candidates(members: &[Vec<bool>], hasher: &mut DefaultHasher) -> Vec<Cell> {
    let mut candidates = Vec::new();
    for (row, cells) in members.iter().enumerate() {
        for (col, &member) in cells.iter().enumerate() {
            if member {
                row.hash(hasher);
                col.hash(hasher);
                if hasher.finish() % 100 < HORIZON_GROUP_PERCENT {
                    candidates.push((row, col));
                }
            }
        }
    }
    candidates
}

/// Splits each connected group of candidates into waterfalls along diagonal lanes.
///
/// Every member lies on an eastern and a western lane, and both families produce candidate
/// waterfalls. Since each cell can only belong to one, they are claimed greedily: largest lane
/// first (across both families), first come first served. A claimed waterfall may then be
/// internally disconnected (some members were taken by larger lanes), so it is split again by
/// adjacency.
fn split_into_lanes(grid: CellGrid, groups: &[Vec<Cell>]) -> Vec<Waterfall> {
    let mut waterfalls = Vec::new();
    for group in groups {
        // Ordered maps, so equal-sized lanes claim cells in the same order on every run.
        let mut lanes: BTreeMap<LaneId, Vec<Cell>> = BTreeMap::new();
        for &cell in group {
            for direction in [LaneDirection::East, LaneDirection::West] {
                lanes
                    .entry((direction, direction.lane_of(cell)))
                    .or_default()
                    .push(cell);
            }
        }

        // Largest first; the sort is stable, so ties keep the lanes' order.
        let mut lanes: Vec<Waterfall> = lanes.into_iter().collect();
        lanes.sort_by_key(|(_, cells)| std::cmp::Reverse(cells.len()));

        let mut taken: HashSet<Cell> = HashSet::new();
        for (lane_id, cells) in lanes {
            let free: Vec<Cell> = cells
                .into_iter()
                .filter(|cell| taken.insert(*cell))
                .collect();
            for component in grid.connected_components(&free) {
                waterfalls.push((lane_id, component));
            }
        }
    }
    waterfalls
}

/// Cuts waterfalls at band boundaries every [`HORIZON_BAND_ROWS`] rows.
///
/// Where a waterfall crosses a band boundary it is cut there with [`HORIZON_BAND_CUT_CHANCE`]
/// probability (split by adjacency again, in case a piece falls apart). The pieces keep their lane,
/// so chaining can still join them to waterfalls of the other direction, but never back to each
/// other.
fn cut_at_bands(
    grid: CellGrid,
    waterfalls: Vec<Waterfall>,
    hasher: &mut DefaultHasher,
) -> Vec<Waterfall> {
    let mut cut = Vec::new();
    for (lane_id, cells) in waterfalls {
        let mut bands: BTreeMap<usize, Vec<Cell>> = BTreeMap::new();
        for &cell in &cells {
            bands
                .entry(cell.0 / HORIZON_BAND_ROWS)
                .or_default()
                .push(cell);
        }
        // Consecutive bands stay in one piece unless the boundary between them is cut.
        let mut pieces: Vec<Vec<Cell>> = Vec::new();
        for (band, band_cells) in bands {
            let cut_here = !pieces.is_empty() && {
                (cells[0], band).hash(hasher);
                hasher.finish() % HORIZON_BAND_CUT_CHANCE.1 < HORIZON_BAND_CUT_CHANCE.0
            };
            match pieces.last_mut() {
                Some(piece) if !cut_here => piece.extend(band_cells),
                _ => pieces.push(band_cells),
            }
        }
        for piece in pieces {
            for component in grid.connected_components(&piece) {
                cut.push((lane_id, component));
            }
        }
    }
    cut
}

/// Joins east and west waterfalls that meet end to end into chains.
///
/// Waterfalls A and B are joined when they run in opposite directions and an end of A lies on B's
/// lane right next to an end of B: it would have continued B had A not claimed it. Each end joins
/// at most once and both joins of a waterfall must be of one kind ([`ChainKind`]), so a chain is a
/// single zigzag running either down the image or sideways, never a branching cluster or one that
/// turns.
fn chain_waterfalls(grid: CellGrid, waterfalls: Vec<Waterfall>) -> Vec<Vec<Cell>> {
    let group_of: HashMap<Cell, usize> = waterfalls
        .iter()
        .enumerate()
        .flat_map(|(index, (_, cells))| cells.iter().map(move |&cell| (cell, index)))
        .collect();
    let ends = waterfall_ends(grid, &waterfalls, &group_of);

    let mut chains = DisjointSet::new(waterfalls.len());
    let mut used_ends: HashSet<Cell> = HashSet::new();
    let mut join_kind: Vec<Option<ChainKind>> = vec![None; waterfalls.len()];
    for (a, ((direction_a, _), _)) in waterfalls.iter().enumerate() {
        let Some((top_a, bottom_a)) = ends[a] else {
            continue;
        };
        let other = direction_a.opposite();
        for end in [top_a, bottom_a] {
            if used_ends.contains(&end) {
                continue;
            }
            let join = grid.neighbours(end).into_iter().find_map(|next| {
                let &b = group_of.get(&next)?;
                let ((direction_b, lane_b), _) = waterfalls[b];
                let (top_b, bottom_b) = ends[b]?;
                // Bottom to top runs down the image; two bottoms or two tops
                // join an east end to a west end, running sideways.
                let kind = if (end == bottom_a) == (next == bottom_b) {
                    ChainKind::Sideways
                } else {
                    ChainKind::Down
                };
                let fits = |existing: Option<ChainKind>| existing.is_none_or(|k| k == kind);
                (b != a
                    && direction_b == other
                    && lane_b == other.lane_of(end)
                    && (next == top_b || next == bottom_b)
                    && !used_ends.contains(&next)
                    && fits(join_kind[a])
                    && fits(join_kind[b]))
                .then_some((b, next, kind))
            });
            let Some((b, next, kind)) = join else {
                continue;
            };
            if !chains.union(a, b) {
                continue; // would close a loop
            }
            used_ends.extend([end, next]);
            join_kind[a] = Some(kind);
            join_kind[b] = Some(kind);
        }
    }

    let mut chained: BTreeMap<usize, Vec<Cell>> = BTreeMap::new();
    for (index, (_, cells)) in waterfalls.into_iter().enumerate() {
        chained.entry(chains.root(index)).or_default().extend(cells);
    }
    chained.into_values().collect()
}

/// Each waterfall's top and bottom ends, or `None` for a single cell. An end has at most one
/// neighbour in its own waterfall (waterfalls are paths).
fn waterfall_ends(
    grid: CellGrid,
    waterfalls: &[Waterfall],
    group_of: &HashMap<Cell, usize>,
) -> Vec<Option<(Cell, Cell)>> {
    let is_end = |group: usize, cell: Cell| {
        grid.neighbours(cell)
            .into_iter()
            .filter(|next| group_of.get(next) == Some(&group))
            .count()
            <= 1
    };
    waterfalls
        .iter()
        .enumerate()
        .map(|(index, ((direction, _), cells))| {
            if cells.len() < 2 {
                return None;
            }
            let key = |cell: &&Cell| direction.down_key(**cell);
            let path_ends = || cells.iter().filter(|&&cell| is_end(index, cell));
            Some((*path_ends().min_by_key(key)?, *path_ends().max_by_key(key)?))
        })
        .collect()
}

/// Union-find over waterfall indices.
struct DisjointSet {
    parent: Vec<usize>,
}

impl DisjointSet {
    fn new(len: usize) -> Self {
        Self {
            parent: (0..len).collect(),
        }
    }

    fn root(&mut self, mut i: usize) -> usize {
        while self.parent[i] != i {
            self.parent[i] = self.parent[self.parent[i]];
            i = self.parent[i];
        }
        i
    }

    /// Joins the sets of `a` and `b`; `false` when they already were one.
    fn union(&mut self, a: usize, b: usize) -> bool {
        let (root_a, root_b) = (self.root(a), self.root(b));
        if root_a == root_b {
            return false;
        }
        self.parent[root_a] = root_b;
        true
    }
}
