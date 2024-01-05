//! Tangent.

use crate::common::consts::ONE;
use crate::common::consts::TRIG_EXP_THRES;
use crate::common::consts::TWO;
use crate::common::util::calc_add_cost;
use crate::common::util::calc_mul_cost;
use crate::common::util::round_p;
use crate::defs::Error;
use crate::defs::RoundingMode;
use crate::num::BigFloatNumber;
use crate::ops::consts::Consts;
use crate::ops::series::series_cost_optimize;
use crate::ops::series::ArgReductionEstimator;
use crate::ops::series::PolycoeffGen;
use crate::ops::util::compute_small_exp;
use crate::Exponent;
use crate::EXPONENT_MIN;
use crate::WORD_BIT_SIZE;

// Polynomial coefficient generator (for tan it only used for cost estmation).
struct TanPolycoeffGen {
    iter_cost: usize,
}

impl TanPolycoeffGen {
    fn new(p: usize) -> Result<Self, Error> {
        let iter_cost =
            9 * calc_mul_cost(p) + 2 * (calc_add_cost(p) + calc_add_cost(WORD_BIT_SIZE));

        Ok(TanPolycoeffGen { iter_cost })
    }
}

impl PolycoeffGen for TanPolycoeffGen {
    fn next(&mut self, _rm: RoundingMode) -> Result<&BigFloatNumber, Error> {
        Ok(&ONE)
    }

    #[inline]
    fn iter_cost(&self) -> usize {
        self.iter_cost
    }
}

struct TanArgReductionEstimator {}

impl ArgReductionEstimator for TanArgReductionEstimator {
    /// Estimates cost of reduction n times for number with precision p.
    fn reduction_cost(n: usize, p: usize) -> u64 {
        let cost_mul = calc_mul_cost(p);
        let cost_add = calc_add_cost(p);
        n as u64 * (2 * cost_mul + cost_add) as u64
    }

    /// Given m, the negative power of 2 of a number, returns the negative power of 2 if reduction is applied n times.
    #[inline]
    fn reduction_effect(n: usize, m: isize) -> usize {
        (n as isize + m) as usize
    }
}

impl BigFloatNumber {
    /// Computes the tangent of a number with precision `p`. The result is rounded using the rounding mode `rm`.
    /// This function requires constants cache `cc` for computing the result.
    /// Precision is rounded upwards to the word size.
    ///
    /// ## Errors
    ///
    ///  - ExponentOverflow: the result is too large or too small number.
    ///  - MemoryAllocation: failed to allocate memory.
    ///  - InvalidArgument: the precision is incorrect.
    pub fn tan(&self, p: usize, rm: RoundingMode, cc: &mut Consts) -> Result<Self, Error> {
        let p = round_p(p);

        if self.is_zero() {
            return Self::new2(p, self.sign(), self.inexact());
        }

        let mut p_inc = WORD_BIT_SIZE;
        let mut p_wrk = p.max(self.mantissa_max_bit_len());

        compute_small_exp!(self, self.exponent() as isize * 2 - 1, false, p_wrk, p, rm);

        p_wrk += p_inc;

        let mut add_p = (3 - TRIG_EXP_THRES) as usize;
        loop {
            let mut x = self.clone()?;

            let p_x = p_wrk + add_p;
            x.set_precision(p_x, RoundingMode::None)?;

            x = x.reduce_trig_arg(cc, RoundingMode::None)?;

            let (t, _) = x.trig_arg_pi_proximity(cc, RoundingMode::None)?;
            if add_p < t {
                add_p = t;
            } else {
                let mut ret = x.tan_series(RoundingMode::None)?;

                if ret.try_set_precision(p, rm, p_wrk)? {
                    ret.set_inexact(ret.inexact() | self.inexact());
                    break Ok(ret);
                }

                p_wrk += p_inc;
                p_inc = round_p(p_wrk / 5);
            }
        }
    }

    fn tan_series(mut self, rm: RoundingMode) -> Result<Self, Error> {
        let p = self.mantissa_max_bit_len();

        let polycoeff_gen = TanPolycoeffGen::new(p)?;
        let (reduction_times, _niter, e_eff) = series_cost_optimize::<TanArgReductionEstimator>(
            p,
            &polycoeff_gen,
            -(self.exponent() as isize),
            1,
            true,
        );

        let add_prec = reduction_times as isize * 4 + 9 - e_eff as isize;
        let p_arg = p + if add_prec > 0 { add_prec as usize } else { 0 };
        self.set_precision(p_arg, rm)?;

        let arg_holder;
        let arg = if reduction_times > 0 {
            arg_holder = self.tan_arg_reduce(reduction_times)?;
            &arg_holder
        } else {
            &self
        };

        let ret = Self::tan_series_run(arg, rm)?;

        if reduction_times > 0 {
            ret.tan_arg_restore(reduction_times, rm)
        } else {
            Ok(ret)
        }
    }

    /// Tangent series
    fn tan_series_run(&self, rm: RoundingMode) -> Result<Self, Error> {
        //  sin + cos series combined
        // tan(x) = x * (((3! - x^2 * 1!) * 5! + x^4 * 3!) * 7! - ...) / (((2! - x^2 * 1!) * 4! + x^4 * 2!) * 6! - ...) / (3*5*7*...)

        let p = self.mantissa_max_bit_len();
        let mut xx = self.mul(self, p, rm)?;
        xx.inv_sign();
        let mut xxacc = BigFloatNumber::from_word(1, 1)?;
        let mut fct = BigFloatNumber::from_word(2, 1)?;
        let mut inc = BigFloatNumber::from_word(2, 1)?;
        let mut q1 = BigFloatNumber::from_word(1, 1)?;
        let mut p1 = BigFloatNumber::from_word(1, 1)?;
        let mut q2 = BigFloatNumber::from_word(1, 1)?;
        let mut p2 = BigFloatNumber::from_word(1, 1)?;

        while fct.exponent() as isize - (xxacc.exponent() as isize) <= p as isize {
            // -x^2, +x^4, -x^6, ...
            xxacc = xxacc.mul(&xx, p, rm)?;

            // cos
            p1 = p1.mul(&fct, p, rm)?;
            let n1 = xxacc.mul(&q1, p, rm)?;
            p1 = p1.add(&n1, p, rm)?;

            q1 = q1.mul(&fct, p, rm)?;

            inc = inc.add(&ONE, inc.mantissa_max_bit_len(), rm)?;
            if fct.mantissa_max_bit_len() < p {
                fct = fct.mul_full_prec(&inc)?;
            } else {
                fct = fct.mul(&inc, p, rm)?;
            }

            // sin
            p2 = p2.mul(&fct, p, rm)?;
            let n1 = xxacc.mul(&q2, p, rm)?;
            p2 = p2.add(&n1, p, rm)?;

            q2 = q2.mul(&fct, p, rm)?;

            inc = inc.add(&ONE, inc.mantissa_max_bit_len(), rm)?;
            if fct.mantissa_max_bit_len() < p {
                fct = fct.mul_full_prec(&inc)?;
            } else {
                fct = fct.mul(&inc, p, rm)?;
            }
        }

        let n0 = p2.mul(&q1, p, rm)?;
        let n1 = n0.mul(self, p, rm)?;
        let n2 = p1.mul(&q2, p, rm)?;

        let mut ret = n1.div(&n2, p, rm)?;
        ret.set_inexact(true);

        Ok(ret)
    }

    // reduce argument n times.
    fn tan_arg_reduce(&self, n: usize) -> Result<Self, Error> {
        // tan(3*x) = 3*tan(x) - tan(x)^3 / (1 - 3*tan(x)^2)
        let mut ret = self.clone()?;
        let p = ret.mantissa_max_bit_len();
        if ret.exponent() < EXPONENT_MIN + n as Exponent {
            ret.set_exponent(EXPONENT_MIN);
            for _ in 0..n - (ret.exponent() - EXPONENT_MIN) as usize {
                ret = ret.div(&TWO, p, RoundingMode::FromZero)?;
            }
        } else {
            ret.set_exponent(ret.exponent() - n as Exponent);
        }
        Ok(ret)
    }

    // restore value for the argument reduced n times.
    fn tan_arg_restore(&self, n: usize, rm: RoundingMode) -> Result<Self, Error> {
        // tan(2*x) = 2*tan(x) / (1 - tan(x)^2)

        let mut val = self.clone()?;
        let p = val.mantissa_max_bit_len();

        for _ in 0..n {
            let val_sq = val.mul(&val, p, rm)?;
            let q = ONE.sub(&val_sq, p, rm)?;
            val.set_exponent(val.exponent() + 1);
            val = val.div(&q, p, rm)?;
        }

        Ok(val)
    }
}

#[cfg(test)]
mod tests {

    use crate::common::util::random_subnormal;

    use super::*;

    #[test]
    fn test_tangent() {
        let p = 320;
        let mut cc = Consts::new().unwrap();
        let rm = RoundingMode::ToEven;
        let mut n1 = BigFloatNumber::from_word(2, p).unwrap();
        n1.set_exponent(0);
        let _n2 = n1.tan(p, rm, &mut cc).unwrap();
        //println!("{:?}", n2.format(crate::Radix::Dec, rm).unwrap());

        // asymptotic & extrema testing
        let mut half_pi = cc.pi_num(128, RoundingMode::None).unwrap();
        half_pi.set_exponent(1);
        half_pi.set_precision(p, RoundingMode::None).unwrap();

        let n2 = half_pi.tan(p, rm, &mut cc).unwrap();
        let n3 = BigFloatNumber::parse(
            "3.1F0B46DCBD63D29899ECF829DA54DE0EE0852B2569B572B793E50817CEF4C77D959712B45E2B7E4C_e+20",
            crate::Radix::Hex,
            p,
            RoundingMode::None, &mut cc,
        )
        .unwrap();

        assert!(n2.cmp(&n3) == 0);

        // large exponent
        half_pi.set_exponent(256);
        let n2 = half_pi.tan(p, rm, &mut cc).unwrap();
        let n3 = BigFloatNumber::parse("4.ECDEC5EF3A1EA5339A46BC0C490F52A86A033C56BCDD413E36C657EB7757F073500B013B9A7B43C0_e+0", crate::Radix::Hex, p, RoundingMode::None, &mut cc).unwrap();

        assert!(n2.cmp(&n3) == 0);

        let d3 = BigFloatNumber::min_positive(p).unwrap();
        let zero = BigFloatNumber::new(1).unwrap();
        let n1 = random_subnormal(p);

        assert!(d3.tan(p, rm, &mut cc).unwrap().cmp(&d3) == 0);
        assert!(zero.tan(p, rm, &mut cc).unwrap().is_zero());
        assert!(n1.tan(p, rm, &mut cc).unwrap().cmp(&n1) == 0);
    }

    #[ignore]
    #[test]
    #[cfg(feature = "std")]
    fn tan_perf() {
        let p = 160;
        let mut cc = Consts::new().unwrap();
        let mut n = vec![];
        for _ in 0..10000 {
            n.push(BigFloatNumber::random_normal(p, 0, 5).unwrap());
        }

        for _ in 0..5 {
            let start_time = std::time::Instant::now();
            for ni in n.iter() {
                let _f = ni.tan(p, RoundingMode::ToEven, &mut cc).unwrap();
            }
            let time = start_time.elapsed();
            println!("{}", time.as_millis());
        }
    }
}
