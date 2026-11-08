//! The statistics of a comparison between two compilers.
//!
//! The harnesses measure the same thing several times under each compiler and compare. The
//! numbers are noisy, so a comparison uses medians, not means, and says how sure it is. This
//! crate holds the arithmetic that rucc-kernel and rucc-postgres had each written for themselves:
//!
//! - [`median`], [`quantile`], [`percentile`] and [`spread`] summarise one set of samples.
//! - [`ratio`] and [`interval`] compare two sets: the ratio of the medians, and a seeded bootstrap
//!   interval for it that comes out the same on every run.
//! - [`geometric_mean`] combines the ratios of several measures.
//! - [`verdict`] says in words whether one set differs from another by more than the noise.
//! - [`Gate`] turns a set of ratios into a pass or a fail.
//!
//! Every function takes plain slices of `f64`. None of them panics on empty input: they return
//! `None` instead.

use std::collections::BTreeMap;

/// The median, or `None` for no numbers.
#[must_use]
pub fn median(values: &[f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let sorted = sorted(values);
    let half = sorted.len() / 2;
    Some(if sorted.len() % 2 == 1 {
        sorted[half]
    } else {
        f64::midpoint(sorted[half - 1], sorted[half])
    })
}

/// A copy of the values in ascending order.
#[must_use]
pub fn sorted(values: &[f64]) -> Vec<f64> {
    let mut out = values.to_vec();
    out.sort_by(f64::total_cmp);
    out
}

/// The value a fraction of the way through sorted samples, between the two nearest when it falls
/// between them, as R and numpy compute quantiles by default. `None` for no samples.
#[must_use]
pub fn quantile(sorted: &[f64], fraction: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let fraction = fraction.clamp(0.0, 1.0);
    #[allow(clippy::cast_precision_loss)]
    let at = (sorted.len() - 1) as f64 * fraction;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let below = at.floor() as usize;
    let above = (below + 1).min(sorted.len() - 1);
    Some(sorted[below] + (at - at.floor()) * (sorted[above] - sorted[below]))
}

/// The nearest rank percentile of sorted samples, `p` from 0 to 100: always one of the samples.
/// `None` for no samples.
#[must_use]
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
pub fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = (p / 100.0 * sorted.len() as f64).ceil() as usize;
    Some(sorted[rank.clamp(1, sorted.len()) - 1])
}

/// The median of a set of samples and the quartiles either side of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spread {
    /// The first quartile.
    pub low: f64,
    /// The median.
    pub median: f64,
    /// The third quartile.
    pub high: f64,
}

impl Spread {
    /// The interquartile range.
    #[must_use]
    pub fn range(&self) -> f64 {
        self.high - self.low
    }
}

/// The median and quartiles of some samples, or `None` when there are none.
#[must_use]
pub fn spread(samples: &[f64]) -> Option<Spread> {
    let sorted = sorted(samples);
    Some(Spread {
        low: quantile(&sorted, 0.25)?,
        median: quantile(&sorted, 0.5)?,
        high: quantile(&sorted, 0.75)?,
    })
}

/// The median of `other` over the median of `reference`. `None` when either is empty or the
/// reference median is not above zero.
#[must_use]
pub fn ratio(reference: &[f64], other: &[f64]) -> Option<f64> {
    let (r, o) = (median(reference)?, median(other)?);
    (r > 0.0).then(|| o / r)
}

/// The geometric mean of some ratios, or `None` when there are none or one is not above zero.
#[must_use]
pub fn geometric_mean(ratios: &[f64]) -> Option<f64> {
    if ratios.is_empty() || ratios.iter().any(|r| *r <= 0.0 || !r.is_finite()) {
        return None;
    }
    let logs: f64 = ratios.iter().map(|r| r.ln()).sum();
    #[allow(clippy::cast_precision_loss)]
    Some((logs / ratios.len() as f64).exp())
}

/// How many times the samples are resampled for an [`interval`].
pub const RESAMPLES: usize = 2000;

/// The seed of the resampling, fixed so that the same samples always give the same interval.
pub const SEED: u64 = 0x9e37_79b9_7f4a_7c15;

/// A small pseudo random sequence: xorshift64. Enough to spread the picks of a bootstrap, and the
/// same every time from the same seed. Not for anything that needs real randomness.
#[derive(Debug, Clone)]
pub struct Xorshift(u64);

impl Xorshift {
    /// A sequence from a seed. A zero seed would give only zeros, so it is replaced by [`SEED`].
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(if seed == 0 { SEED } else { seed })
    }

    /// The next number.
    pub fn next_u64(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A number below `bound`, which must not be zero.
    #[allow(clippy::cast_possible_truncation)]
    pub fn below(&mut self, bound: usize) -> usize {
        (self.next_u64() % bound as u64) as usize
    }
}

/// The interval of the ratio of medians at a confidence level such as 0.95: resample each set
/// with replacement, take the ratio of the medians each time, and keep the two tail percentiles.
/// Seeded with [`SEED`], so the same samples give the same interval on every run.
#[must_use]
pub fn interval(reference: &[f64], other: &[f64], confidence: f64) -> Option<(f64, f64)> {
    ratio(reference, other)?;
    let mut random = Xorshift::new(SEED);
    let mut pick = |values: &[f64]| -> Vec<f64> {
        (0..values.len())
            .map(|_| values[random.below(values.len())])
            .collect()
    };
    let ratios: Vec<f64> = (0..RESAMPLES)
        .filter_map(|_| {
            let r = pick(reference);
            let o = pick(other);
            ratio(&r, &o)
        })
        .collect();
    let ratios = sorted(&ratios);
    let tail = (1.0 - confidence.clamp(0.0, 1.0)) / 2.0;
    let at = |q: f64| {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let i = ((ratios.len() - 1) as f64 * q).round() as usize;
        ratios[i]
    };
    (!ratios.is_empty()).then(|| (at(tail), at(1.0 - tail)))
}

/// What `b` comes to against `a` for one measure: how much faster or slower, or "no measurable
/// difference" when the medians are closer than the two interquartile ranges added together.
#[must_use]
pub fn verdict(higher_is_better: bool, a: Spread, b: Spread) -> String {
    if (b.median - a.median).abs() <= a.range() + b.range() {
        return "no measurable difference".to_string();
    }
    let speed = if higher_is_better {
        b.median / a.median
    } else {
        a.median / b.median
    };
    if speed >= 1.0 {
        format!("{:.1}% faster", (speed - 1.0) * 100.0)
    } else {
        format!("{:.1}% slower", (1.0 - speed) * 100.0)
    }
}

/// The samples of one measure under both compilers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pair {
    /// The reference compiler's samples.
    pub reference: Vec<f64>,
    /// The samples of the compiler under test.
    pub other: Vec<f64>,
}

/// A performance gate over a set of measures, where a ratio is other over reference and lower is
/// better, as with time.
#[derive(Debug, Clone, PartialEq)]
pub struct Gate {
    /// The most the geometric mean of the ratios can be.
    pub mean_limit: f64,
    /// The most the top of one measure's interval can be, and how far the control's ratio can be
    /// from 1 either way.
    pub one_limit: f64,
    /// The confidence of each interval.
    pub confidence: f64,
    /// A measure that both compilers should run at the same speed, such as `memcpy`, which says
    /// whether the machine was quiet. It is left out of the mean.
    pub control: Option<String>,
}

impl Default for Gate {
    /// The gate of rucc-kernel K8: a mean within 2%, every measure within 5% at 95%.
    fn default() -> Self {
        Self {
            mean_limit: 1.02,
            one_limit: 1.05,
            confidence: 0.95,
            control: None,
        }
    }
}

/// What a [`Gate`] found.
#[derive(Debug, Clone, PartialEq)]
pub struct Judgement {
    /// The geometric mean of the ratios, without the control.
    pub mean: Option<f64>,
    /// Whether the control stayed within the limit, or `true` when there is no control.
    pub quiet: bool,
    /// The measures whose interval goes above the limit, or that had no interval.
    pub over: Vec<String>,
    /// Whether the whole set passed.
    pub passed: bool,
}

impl Gate {
    /// Judge a set of measures by name.
    #[must_use]
    pub fn judge(&self, measures: &BTreeMap<String, Pair>) -> Judgement {
        let is_control = |name: &str| self.control.as_deref() == Some(name);
        let ratios: Vec<f64> = measures
            .iter()
            .filter(|(name, _)| !is_control(name))
            .filter_map(|(_, p)| ratio(&p.reference, &p.other))
            .collect();
        let mean = geometric_mean(&ratios);
        let quiet = match &self.control {
            None => true,
            Some(name) => measures
                .get(name)
                .and_then(|p| ratio(&p.reference, &p.other))
                .is_some_and(|r| (1.0 / self.one_limit..=self.one_limit).contains(&r)),
        };
        let over: Vec<String> = measures
            .iter()
            .filter(|(name, _)| !is_control(name))
            .filter(|(_, p)| {
                !interval(&p.reference, &p.other, self.confidence)
                    .is_some_and(|(_, high)| high <= self.one_limit)
            })
            .map(|(name, _)| name.clone())
            .collect();
        let passed = quiet && over.is_empty() && mean.is_some_and(|m| m <= self.mean_limit);
        Judgement {
            mean,
            quiet,
            over,
            passed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn the_median_takes_the_middle_or_the_midpoint() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), Some(2.5));
    }

    #[test]
    fn quantiles_interpolate_as_numpy_does() {
        let s = [1.0, 2.0, 3.0, 4.0];
        assert!(close(quantile(&s, 0.25).unwrap(), 1.75));
        assert!(close(quantile(&s, 0.5).unwrap(), 2.5));
        assert!(close(quantile(&s, 1.0).unwrap(), 4.0));
        assert_eq!(quantile(&[], 0.5), None);
    }

    #[test]
    fn percentiles_are_taken_by_nearest_rank() {
        let s: Vec<f64> = (1..=10).map(f64::from).collect();
        assert_eq!(percentile(&s, 50.0), Some(5.0));
        assert_eq!(percentile(&s, 90.0), Some(9.0));
        assert_eq!(percentile(&s, 99.0), Some(10.0));
        assert_eq!(percentile(&s, 0.0), Some(1.0));
        assert_eq!(percentile(&[], 50.0), None);
    }

    #[test]
    fn a_spread_has_quartiles_and_a_range() {
        let s = spread(&[5.0, 1.0, 3.0, 2.0, 4.0]).unwrap();
        assert_eq!((s.low, s.median, s.high), (2.0, 3.0, 4.0));
        assert!(close(s.range(), 2.0));
        assert_eq!(spread(&[]), None);
    }

    #[test]
    fn the_ratio_is_of_the_medians() {
        assert_eq!(ratio(&[10.0, 10.0, 99.0], &[11.0, 0.0, 11.0]), Some(1.1));
        assert_eq!(ratio(&[], &[1.0]), None);
        assert_eq!(ratio(&[0.0], &[1.0]), None);
    }

    #[test]
    fn the_geometric_mean_of_reciprocals_is_one() {
        assert!(close(geometric_mean(&[2.0, 0.5]).unwrap(), 1.0));
        assert!(close(geometric_mean(&[1.0, 4.0]).unwrap(), 2.0));
        assert_eq!(geometric_mean(&[]), None);
        assert_eq!(geometric_mean(&[1.0, 0.0]), None);
    }

    #[test]
    fn the_interval_is_the_same_on_every_run_and_holds_the_ratio() {
        let reference = [10.0, 10.2, 9.9, 10.1, 10.0, 9.8, 10.3];
        let other = [10.5, 10.4, 10.6, 10.5, 10.7, 10.3, 10.5];
        let first = interval(&reference, &other, 0.95).unwrap();
        let again = interval(&reference, &other, 0.95).unwrap();
        assert_eq!(first, again);
        let r = ratio(&reference, &other).unwrap();
        assert!(first.0 <= r && r <= first.1, "{first:?} {r}");
        assert!(
            first.0 > 1.0,
            "a clear 5% slowdown stays above 1: {first:?}"
        );
        assert_eq!(interval(&[], &other, 0.95), None);
    }

    #[test]
    fn the_sequence_is_fixed_by_its_seed() {
        let mut a = Xorshift::new(SEED);
        let mut b = Xorshift::new(SEED);
        let first: Vec<u64> = (0..4).map(|_| a.next_u64()).collect();
        let second: Vec<u64> = (0..4).map(|_| b.next_u64()).collect();
        assert_eq!(first, second);
        assert_ne!(Xorshift::new(0).next_u64(), 0);
    }

    #[test]
    fn a_verdict_needs_more_than_the_noise() {
        let a = spread(&[100.0, 101.0, 99.0, 100.0]).unwrap();
        let b = spread(&[100.5, 101.5, 99.5, 100.5]).unwrap();
        assert_eq!(verdict(false, a, b), "no measurable difference");
        let c = spread(&[110.0, 110.0, 110.0, 110.0]).unwrap();
        assert_eq!(verdict(false, a, c), "9.1% slower");
        assert_eq!(verdict(true, a, c), "10.0% faster");
    }

    fn pair(reference: &[f64], other: &[f64]) -> Pair {
        Pair {
            reference: reference.to_vec(),
            other: other.to_vec(),
        }
    }

    #[test]
    fn the_gate_passes_a_close_run_and_fails_a_slow_one() {
        let gate = Gate {
            control: Some("memcpy".into()),
            ..Gate::default()
        };
        let same = [10.0, 10.1, 9.9, 10.0, 10.05];
        let mut measures = BTreeMap::new();
        measures.insert("memcpy".to_string(), pair(&same, &same));
        measures.insert("fork".to_string(), pair(&same, &same));
        let judged = gate.judge(&measures);
        assert!(judged.passed, "{judged:?}");
        assert!(close(judged.mean.unwrap(), 1.0));

        measures.insert(
            "exec".to_string(),
            pair(&same, &[12.0, 12.1, 11.9, 12.0, 12.05]),
        );
        let judged = gate.judge(&measures);
        assert!(!judged.passed);
        assert_eq!(judged.over, ["exec"]);
    }

    #[test]
    fn a_noisy_control_fails_the_gate() {
        let gate = Gate {
            control: Some("memcpy".into()),
            ..Gate::default()
        };
        let mut measures = BTreeMap::new();
        measures.insert("memcpy".to_string(), pair(&[10.0], &[11.0]));
        measures.insert("fork".to_string(), pair(&[10.0], &[10.0]));
        let judged = gate.judge(&measures);
        assert!(!judged.quiet);
        assert!(!judged.passed);
    }
}
