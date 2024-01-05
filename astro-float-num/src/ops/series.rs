//! Power series computation appliance.

use crate::common::util::calc_add_cost;
use crate::common::util::calc_mul_cost;
use crate::common::util::log2_floor;
use crate::common::util::nroot_int;
use crate::common::util::sqrt_int;
use crate::defs::Error;
use crate::defs::RoundingMode;
use crate::num::BigFloatNumber;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

const MAX_CACHE: usize = 128;
const RECT_ITER_THRESHOLD: usize = MAX_CACHE / 10 * 9;

//
// Public part
//

/// Generator of polynomial coefficients.
pub(crate) trait PolycoeffGen {
    /// Returns the next polynomial coefficient value.
    fn next(&mut self, rm: RoundingMode) -> Result<&BigFloatNumber, Error>;

    /// Returns the cost of one call to next if numbers have precision p.
    fn iter_cost(&self) -> usize;

    /// Returns true if coefficient is divizor.
    fn is_div(&self) -> bool {
        false
    }
}

/// Estimate how argument reduction influences cost.
pub trait ArgReductionEstimator {
    /// Estimates cost of reduction n times for number with precision p.
    fn reduction_cost(n: usize, p: usize) -> u64;

    /// Given m, the negative power of 2 of a number, returns the negative power of 2 if reduction is applied n times.
    fn reduction_effect(n: usize, m: isize) -> usize;
}

/// Compute the number of reductions required for the best performance.
/// p is the number precision
/// polycoeff_gen is the polynomial coefficient ganerator
/// m is the negative exponent of the number.
/// pwr_step - increment of power of x in each iteration
/// ext - if true use series step cost directly
pub(crate) fn series_cost_optimize<S: ArgReductionEstimator>(
    p: usize,
    polycoeff_gen: &impl PolycoeffGen,
    m: isize,
    pwr_step: usize,
    ext: bool,
) -> (usize, usize, usize) {
    let reduction_num_step = log2_floor(p) / 2;

    let mut reduction_times = if reduction_num_step as isize > m {
        (reduction_num_step as isize - m) as usize
    } else {
        0
    };

    let mut cost1 = u64::MAX;

    loop {
        let m_eff = S::reduction_effect(reduction_times, m);
        let niter = series_niter(p, m_eff) / pwr_step;
        let cost2 = if ext {
            polycoeff_gen.iter_cost() as u64 * niter as u64
        } else {
            series_cost(niter, p, polycoeff_gen)
        } + S::reduction_cost(reduction_times, p);

        if cost2 < cost1 {
            cost1 = cost2;
            reduction_times += reduction_num_step;
        } else {
            return (reduction_times - reduction_num_step, niter, m_eff);
        }
    }
}

pub(crate) fn series_run<T: PolycoeffGen>(
    acc: BigFloatNumber,
    x_first: BigFloatNumber,
    x_step: BigFloatNumber,
    niter: usize,
    polycoeff_gen: &mut T,
) -> Result<BigFloatNumber, Error> {
    let mut ret = if x_first.is_zero() || x_step.is_zero() {
        series_compute_fast(acc, x_first, polycoeff_gen)
    } else if niter >= RECT_ITER_THRESHOLD {
        series_rectangular(niter, acc, x_first, x_step, polycoeff_gen)
    } else if polycoeff_gen.is_div() {
        series_linear(acc, x_first, x_step, polycoeff_gen)
    } else {
        series_horner(acc, x_first, x_step, polycoeff_gen)
    }?;

    ret.set_inexact(true);

    Ok(ret)
}

//
// Private part
//

/// Estimate of the number of series iterations.
/// p is the precision, m is the negative power of x
/// (i.e. x = f*2^(-m), where 0.5 <= f < 1).
fn series_niter(p: usize, m: usize) -> usize {
    let ln = log2_floor(p);
    let lln = log2_floor(ln);
    p / (ln - lln + m - 2)
}

/// Estimate cost of execution for series.
/// niter is the estimated number of series iterations
/// p is the numbers precision
/// polycoeff_gen is the coefficient generator
fn series_cost<T: PolycoeffGen>(niter: usize, p: usize, polycoeff_gen: &T) -> u64 {
    let cost_mul = calc_mul_cost(p);
    let cost_add = calc_add_cost(p);
    let cost = niter as u64 * (cost_mul + cost_add + polycoeff_gen.iter_cost()) as u64;

    if niter >= RECT_ITER_THRESHOLD {
        // niter * (cost(mul) + cost(add) + cost(polcoeff_gen.next)) + sqrt(niter) * cost(mul)
        // + niter / 10 * (2 * cost(mul) + cost(add) + cost(polcoeff_gen.next))
        cost + sqrt_int(niter as u32) as u64 * cost_mul as u64
            + niter as u64 / 10 * ((cost_mul << 1) + cost_add + polycoeff_gen.iter_cost()) as u64
    } else {
        // niter * (cost(mul) + cost(add) + cost(polycoeff_gen.next))
        cost
    }
}

// x_first or x_step is zero
fn series_compute_fast<T: PolycoeffGen>(
    acc: BigFloatNumber,
    x_first: BigFloatNumber,
    polycoeff_gen: &mut T,
) -> Result<BigFloatNumber, Error> {
    if x_first.is_zero() {
        Ok(acc)
    } else {
        let p = acc
            .mantissa_max_bit_len()
            .max(x_first.mantissa_max_bit_len());

        let is_div = polycoeff_gen.is_div();

        let coeff = polycoeff_gen.next(RoundingMode::None)?;

        let part = if is_div {
            x_first.div(coeff, p, RoundingMode::None)
        } else {
            x_first.mul(coeff, p, RoundingMode::None)
        }?;

        acc.add(&part, p, RoundingMode::None)
    }
}

// Rectangular series.
// p is the result precision
// niter is the estimated number of iterations
// x_first is the first power of x in the series
// x_step is a multiplication factor for each step
// polycoeff_gen is a generator of polynomial coeeficients for the series
// rm is the rounding mode.
// cost: niter * (O(mul) + O(add) + cost(polcoeff_gen.next)) + sqrt(niter) * O(mul) + cost(series_line(remainder))
fn series_rectangular<T: PolycoeffGen>(
    mut niter: usize,
    add: BigFloatNumber,
    x_first: BigFloatNumber,
    x_step: BigFloatNumber,
    polycoeff_gen: &mut T,
) -> Result<BigFloatNumber, Error> {
    debug_assert!(niter >= 4);

    let p = add
        .mantissa_max_bit_len()
        .max(x_first.mantissa_max_bit_len())
        .max(x_step.mantissa_max_bit_len());

    let mut acc = BigFloatNumber::new(p)?;

    // build cache
    let mut cache = Vec::<BigFloatNumber>::new();
    let sqrt_iter = sqrt_int(niter as u32) as usize;
    let cache_sz = MAX_CACHE.min(sqrt_iter);
    cache.try_reserve_exact(cache_sz)?;
    let mut x_pow = x_step.clone()?;

    for _ in 0..cache_sz {
        cache.push(x_pow.clone()?);
        x_pow = x_pow.mul(&x_step, p, RoundingMode::None)?;
    }

    // run computation
    let poly_val = compute_row(p, &cache, polycoeff_gen)?;
    acc = acc.add(&poly_val, p, RoundingMode::None)?;
    let mut terminal_pow = x_pow.clone()?;
    niter -= cache_sz;

    loop {
        let poly_val = compute_row(p, &cache, polycoeff_gen)?;
        let part = poly_val.mul(&terminal_pow, p, RoundingMode::None)?;
        acc = acc.add(&part, p, RoundingMode::None)?;
        terminal_pow = terminal_pow.mul(&x_pow, p, RoundingMode::None)?;
        niter -= cache_sz;

        if niter < cache_sz {
            break;
        }
    }
    drop(cache);

    acc = acc.mul(&x_first, p, RoundingMode::None)?;
    terminal_pow = terminal_pow.mul(&x_first, p, RoundingMode::None)?;
    acc = acc.add(&add, p, RoundingMode::None)?;

    acc = if niter < MAX_CACHE * 10 && !polycoeff_gen.is_div() {
        // probably not too many iterations left
        series_horner(acc, terminal_pow, x_step, polycoeff_gen)
    } else {
        series_linear(acc, terminal_pow, x_step, polycoeff_gen)
    }?;

    Ok(acc)
}

// Linear series
// cost: niter * (2 * O(mul) + O(add) + cost(polcoeff_gen.next))
fn series_linear<T: PolycoeffGen>(
    mut acc: BigFloatNumber,
    x_first: BigFloatNumber,
    x_step: BigFloatNumber,
    polycoeff_gen: &mut T,
) -> Result<BigFloatNumber, Error> {
    let p = acc
        .mantissa_max_bit_len()
        .max(x_first.mantissa_max_bit_len())
        .max(x_step.mantissa_max_bit_len());

    let is_div = polycoeff_gen.is_div();
    let mut x_pow = x_first;

    loop {
        let coeff = polycoeff_gen.next(RoundingMode::None)?;
        let part = if is_div {
            x_pow.div(coeff, p, RoundingMode::None)
        } else {
            x_pow.mul(coeff, p, RoundingMode::None)
        }?;

        acc = acc.add(&part, p, RoundingMode::None)?;

        if part.exponent() as isize <= acc.exponent() as isize - acc.mantissa_max_bit_len() as isize
        {
            break;
        }

        x_pow = x_pow.mul(&x_step, p, RoundingMode::None)?;
    }

    Ok(acc)
}

// compute row of rectangle series.
fn compute_row<T: PolycoeffGen>(
    p: usize,
    cache: &[BigFloatNumber],
    polycoeff_gen: &mut T,
) -> Result<BigFloatNumber, Error> {
    let is_div = polycoeff_gen.is_div();

    let mut acc = BigFloatNumber::new(p)?;
    let coeff = polycoeff_gen.next(RoundingMode::None)?;
    if is_div {
        let r = coeff.reciprocal(p, RoundingMode::None)?;
        acc = acc.add(&r, p, RoundingMode::None)?;
    } else {
        acc = acc.add(coeff, p, RoundingMode::None)?;
    }

    for x_pow in cache {
        let coeff = polycoeff_gen.next(RoundingMode::None)?;
        let add = if is_div {
            x_pow.div(coeff, p, RoundingMode::None)
        } else {
            x_pow.mul(coeff, p, RoundingMode::None)
        }?;
        acc = acc.add(&add, p, RoundingMode::None)?;
    }

    Ok(acc)
}

/// Horner's method
/// cost: niter*(O(mul) + O(add) + cost(polycoeff_gen.next))
fn series_horner<T: PolycoeffGen>(
    add: BigFloatNumber,
    x_first: BigFloatNumber,
    x_step: BigFloatNumber,
    polycoeff_gen: &mut T,
) -> Result<BigFloatNumber, Error> {
    debug_assert!(x_first.exponent() <= 0);
    debug_assert!(x_step.exponent() <= 0);
    debug_assert!(!polycoeff_gen.is_div());

    let p = add
        .mantissa_max_bit_len()
        .max(x_first.mantissa_max_bit_len())
        .max(x_step.mantissa_max_bit_len());

    // determine number of parts and cache plynomial coeffs.
    let mut cache = Vec::<BigFloatNumber>::new();
    let mut x_p = -(x_first.exponent() as isize) - x_step.exponent() as isize;
    let mut coef_p = 0;

    while x_p + coef_p < p as isize - add.exponent() as isize {
        let coeff = polycoeff_gen.next(RoundingMode::None)?;
        coef_p = -coeff.exponent() as isize;
        x_p += -x_step.exponent() as isize;
        cache.push(coeff.clone()?);
    }

    let last_coeff = polycoeff_gen.next(RoundingMode::None)?;
    let mut acc = last_coeff.clone()?;

    for coeff in cache.iter().rev() {
        acc = acc.mul(&x_step, p, RoundingMode::None)?;
        acc = acc.add(coeff, p, RoundingMode::None)?;
    }

    acc = acc.mul(&x_first, p, RoundingMode::None)?;

    add.add(&acc, p, RoundingMode::None)
}

// Is it possbile to make it more effective than series_rect?
// compute series using n dimensions
// p is the result precision
// niter is the estimated number of iterations
// x_first is the first power of x in the series
// x_step is a multiplication factor for each step
// polycoeff_gen is a generator of polynomial coefficients for the series
// rm is the rounding mode.
#[allow(dead_code)]
fn ndim_series<T: PolycoeffGen>(
    n: usize,
    niter: usize,
    add: BigFloatNumber,
    x_factor: BigFloatNumber,
    x_step: BigFloatNumber,
    polycoeff_gen: &mut T,
) -> Result<BigFloatNumber, Error> {
    debug_assert!((2..=8).contains(&n));

    let p = add
        .mantissa_max_bit_len()
        .max(x_factor.mantissa_max_bit_len())
        .max(x_step.mantissa_max_bit_len());

    let mut acc = BigFloatNumber::new(p)?;

    // build cache
    let mut cache = Vec::<BigFloatNumber>::new();
    let cache_dim_sz = nroot_int(niter as u64, n) as usize - 1;
    let cache_dim_sz = cache_dim_sz.min(MAX_CACHE / (n - 1));
    let mut x_pow = x_step.clone()?;

    for _ in 0..n - 1 {
        let cache_step = x_pow.clone()?;

        for _ in 0..cache_dim_sz {
            cache.push(x_pow.clone()?);
            x_pow = x_pow.mul(&cache_step, p, RoundingMode::None)?;
        }
    }

    // run computation
    let poly_val = compute_cube(
        acc.mantissa_max_bit_len(),
        n - 1,
        &cache,
        cache_dim_sz,
        polycoeff_gen,
    )?;
    acc = acc.add(&poly_val, p, RoundingMode::None)?;
    let mut terminal_pow = x_pow.clone()?;

    for _ in 1..cache_dim_sz {
        let poly_val = compute_cube(
            acc.mantissa_max_bit_len(),
            n - 1,
            &cache,
            cache_dim_sz,
            polycoeff_gen,
        )?;
        let part = poly_val.mul(&terminal_pow, p, RoundingMode::None)?;
        acc = acc.add(&part, p, RoundingMode::None)?;
        terminal_pow = terminal_pow.mul(&x_pow, p, RoundingMode::None)?;
    }

    acc = acc.mul(&x_factor, p, RoundingMode::None)?;
    terminal_pow = terminal_pow.mul(&x_factor, p, RoundingMode::None)?;
    acc = acc.add(&add, p, RoundingMode::None)?;

    series_linear(acc, terminal_pow, x_step, polycoeff_gen)
}

#[allow(dead_code)]
fn compute_cube<T: PolycoeffGen>(
    p: usize,
    n: usize,
    cache: &[BigFloatNumber],
    cache_dim_sz: usize,
    polycoeff_gen: &mut T,
) -> Result<BigFloatNumber, Error> {
    if n > 1 {
        let mut acc = BigFloatNumber::new(p)?;
        let cache_dim_sz = cache_dim_sz;
        // no need to multityply the returned coefficient of the first cube by 1.
        let poly_val = compute_cube(p, n - 1, cache, cache_dim_sz, polycoeff_gen)?;
        acc = acc.add(&poly_val, p, RoundingMode::None)?;

        // the remaining require multiplication
        for x_pow in &cache[cache_dim_sz * (n - 1)..cache_dim_sz * n] {
            let poly_val = compute_cube(p, n - 1, cache, cache_dim_sz, polycoeff_gen)?;
            let add = x_pow.mul(&poly_val, p, RoundingMode::None)?;
            acc = acc.add(&add, p, RoundingMode::None)?;
        }

        Ok(acc)
    } else {
        compute_row(p, &cache[..cache_dim_sz], polycoeff_gen)
    }
}
