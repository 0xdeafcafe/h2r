//! Locally mapped users receive obstruction updates in slot order. A call
//! can change a mapping, so only search as far as the next active slot.

pub const USER_COUNT: i32 = 4;

pub fn next_listener<E>(
    after: i32,
    mut mapped: impl FnMut(i32) -> Result<bool, E>,
) -> Result<Option<i32>, E> {
    for user in after.wrapping_add(1)..USER_COUNT {
        if mapped(user)? {
            return Ok(Some(user));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listener_order_tracks_mapping_changes_at_each_callback() {
        let mut users = [true, false, true, false];
        let mut visited = Vec::new();
        let mut previous = -1;
        while let Some(user) = next_listener(previous, |i| Ok::<_, ()>(users[i as usize])).unwrap() {
            visited.push(user);
            if user == 0 {
                users[1] = true;
                users[2] = false;
                users[3] = true;
            }
            previous = user;
        }
        assert_eq!(visited, [0, 1, 3]);
    }

    #[test]
    fn scan_reads_nothing_past_the_first_live_slot() {
        for bits in 0u8..16 {
            for previous in -1..4 {
                let expected = (previous + 1..USER_COUNT).find(|&slot| bits & (1 << slot) != 0);
                let next = next_listener(previous, |slot| {
                    assert!(slot <= expected.unwrap_or(USER_COUNT));
                    Ok::<_, ()>(bits & (1 << slot) != 0)
                });
                assert_eq!(next, Ok(expected));
            }
        }
    }
}
