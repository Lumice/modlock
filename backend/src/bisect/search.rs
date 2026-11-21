//! Pure port of upstream search.ts (GPL-3.0); unsure counts as working.
use super::{ids, key};
use json::{object, JsonValue};
pub fn create(candidates: Vec<String>) -> JsonValue {
    object! {found:json::array![],high:candidates.len(),pool:candidates,low:-1,results:object!{}}
}
pub fn record(state: &mut JsonValue, config: &[String], outcome: &str) {
    state["results"][&key(config)] = outcome.into();
}
/// Pure search; unsure has exactly the same effect as fine.
pub fn next(mut state: JsonValue) -> (JsonValue, Vec<String>, bool) {
    loop {
        let low = state["low"].as_i32().unwrap();
        let high = state["high"].as_i32().unwrap();
        let mut found = ids(&state["found"]);
        let pool = ids(&state["pool"]);
        if low >= high {
            return (state, found, true);
        }
        if low >= 0 && high - low == 1 {
            found.push(pool[high as usize - 1].clone());
            state["found"] = found.into();
            state["pool"] = pool[..high as usize - 1].to_vec().into();
            state["low"] = (-1).into();
            state["high"] = (high - 1).into();
            continue;
        }
        let length = if low < 0 {
            0
        } else {
            ((low + high) / 2) as usize
        };
        found.extend_from_slice(&pool[..length]);
        match state["results"][&key(&found)].as_str() {
            None => return (state, found, false),
            Some("broken") => {
                if length == 0 {
                    return (state, ids(&JsonValue::from(found)), true);
                }
                state["high"] = length.into()
            }
            _ => state["low"] = length.into(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn run(n: usize, culprits: &[usize], unsure: bool) -> usize {
        let candidates = (0..n).map(|i| i.to_string()).collect();
        let mut state = create(candidates);
        let mut launches = 0;
        loop {
            let (s, c, done) = next(state);
            state = s;
            if done {
                let mut actual = c;
                actual.sort();
                let mut want: Vec<_> = culprits.iter().map(usize::to_string).collect();
                want.sort();
                assert_eq!(actual, want);
                return launches;
            }
            launches += 1;
            assert!(launches < 40);
            let broken = culprits.iter().all(|i| c.contains(&i.to_string()));
            record(
                &mut state,
                &c,
                if broken {
                    "broken"
                } else if unsure {
                    "unsure"
                } else {
                    "fine"
                },
            );
        }
    }
    #[test]
    fn single_and_bounds() {
        for n in [1, 2, 100] {
            for culprit in 0..n {
                let launches = run(n, &[culprit], false);
                assert!(launches <= (n.max(2) as f64).log2().ceil() as usize + 2)
            }
        }
    }
    #[test]
    fn pairs_and_unsure() {
        for a in 0..100 {
            let launches = run(100, &[a, (a + 37) % 100], true);
            assert!(launches <= 17)
        }
    }
    #[test]
    fn baseline_broken() {
        assert_eq!(run(100, &[], false), 1);
    }
}
