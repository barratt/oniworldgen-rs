//! Natural cavities used by ONI's ambient spawning, build 744825.
//! Input is managed worldgen simData solidity, not the final physics grid.

/// Return ascending cell indices in non-solid four-connected components of
/// at most 300 cells. Traversal order is immaterial to the resulting set.
pub fn detect(width: usize, height: usize, solid: &[bool]) -> Result<Vec<usize>, String> {
    let count = width.checked_mul(height).ok_or("cavity grid overflow")?;
    if width == 0 || height == 0 || count != solid.len() {
        return Err("invalid cavity grid dimensions".into());
    }
    let mut visited = vec![false; count];
    let mut result = Vec::new();
    let mut pending = Vec::new();
    let mut component = Vec::new();
    for start in 0..count {
        if visited[start] || solid[start] { continue; }
        visited[start] = true;
        pending.push(start);
        component.clear();
        while let Some(cell) = pending.pop() {
            component.push(cell);
            let x = cell % width;
            let neighbors = [
                (x > 0).then(|| cell - 1),
                (x + 1 < width).then(|| cell + 1),
                (cell >= width).then(|| cell - width),
                (cell < count - width).then(|| cell + width),
            ];
            for neighbor in neighbors.into_iter().flatten() {
                if !visited[neighbor] && !solid[neighbor] {
                    visited[neighbor] = true;
                    pending.push(neighbor);
                }
            }
        }
        if component.len() <= 300 { result.extend_from_slice(&component); }
    }
    result.sort_unstable();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn threshold_is_inclusive_and_large_regions_are_fully_visited() {
        let mut solid=vec![false; 603]; solid[300]=true; solid[602]=true;
        assert_eq!(detect(603,1,&solid).unwrap(),(0..300).collect::<Vec<_>>());
    }
    #[test]
    fn rows_do_not_wrap_and_diagonals_do_not_connect() {
        // A 300-cell region plus a diagonal singleton: wrapping would merge
        // them into a rejected 301-cell component.
        let mut solid=vec![true; 602];
        solid[1..301].fill(false); solid[301]=false;
        let expected=(1..301).chain([301]).collect::<Vec<_>>();
        assert_eq!(detect(301,2,&solid).unwrap(),expected);
        assert!(detect(0,2,&[]).is_err());
        assert!(detect(3,2,&[true]).is_err());
    }
}
