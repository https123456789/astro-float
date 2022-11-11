//! Cosine.


use crate::Exponent;
use crate::Sign;
use crate::common::consts::FOUR;
use crate::common::consts::ONE;
use crate::common::consts::THREE;
use crate::common::util::get_add_cost;
use crate::common::util::get_mul_cost;
use crate::num::BigFloatNumber;
use crate::defs::RoundingMode;
use crate::defs::Error;
use crate::ops::series::PolycoeffGen;
use crate::ops::series::ArgReductionEstimator;
use crate::ops::series::series_run;
use crate::ops::series::series_cost_optimize;
use crate::ops::consts::Consts;


const COS_EXP_THRES: Exponent = -32;


// Polynomial coefficient generator.
struct CosPolycoeffGen {
    one_full_p: BigFloatNumber,
    inc: BigFloatNumber,
    fct: BigFloatNumber,
    sign: i8,
    iter_cost: usize,
}

impl CosPolycoeffGen {

    fn new(p: usize) -> Result<Self, Error> {

        let inc = BigFloatNumber::new(1)?;
        let fct = BigFloatNumber::from_word(1, p)?;
        let one_full_p = BigFloatNumber::from_word(1, p)?;

        let iter_cost = (get_mul_cost(p) + get_add_cost(p)) << 1; // 2 * (cost(mul) + cost(add))

        let sign = 1;

        Ok(CosPolycoeffGen {
            one_full_p,
            inc,
            fct,
            sign,
            iter_cost,
        })
    }
}

impl PolycoeffGen for CosPolycoeffGen {

    fn next(&mut self, rm: RoundingMode) -> Result<&BigFloatNumber, Error> {

        self.inc = self.inc.add(&ONE, rm)?;
        let inv_inc = self.one_full_p.div(&self.inc, rm)?;
        self.fct = self.fct.mul(&inv_inc, rm)?;

        self.inc = self.inc.add(&ONE, rm)?;
        let inv_inc = self.one_full_p.div(&self.inc, rm)?;
        self.fct = self.fct.mul(&inv_inc, rm)?;

        self.sign *= -1;
        if self.sign > 0 {
            self.fct.set_sign(Sign::Pos);
        } else {
            self.fct.set_sign(Sign::Neg);
        }

        Ok(&self.fct)
    }

    #[inline]
    fn get_iter_cost(&self) -> usize {
        self.iter_cost
    }
}

struct CosArgReductionEstimator {}

impl ArgReductionEstimator for CosArgReductionEstimator {

    /// Estimates cost of reduction n times for number with precision p.
    fn get_reduction_cost(n: usize, p: usize) -> usize {
        // n * (4 * cost(mul) + 2 * cost(add))
        let cost_mul = get_mul_cost(p);
        let cost_add = get_add_cost(p);
        (n * ((cost_mul << 1) + cost_add )) << 1
    }

    /// Given m, the negative power of 2 of a number, returns the negative power of 2 if reduction is applied n times.
    #[inline]
    fn reduction_effect(n: usize, m: isize) -> usize {
        // n*log2(3) + m
        ((n as isize)*1000/631 + m) as usize
    }
}

impl BigFloatNumber {

    /// Computes the cosine of a number. The result is rounded using the rounding mode `rm`.
    /// This function requires constants cache `cc` for computing the result.
    /// 
    /// ## Errors
    /// 
    ///  - MemoryAllocation: failed to allocate memory.
    pub fn cos(&self, rm: RoundingMode, cc: &mut Consts) -> Result<Self, Error> {

        let mut arg = self.reduce_trig_arg(cc, RoundingMode::None)?;

        let mut ret;

        let arg1 = arg.clone()?;

        ret = arg1.cos_series(RoundingMode::None)?;

        if ret.get_exponent() < COS_EXP_THRES {
            
            // argument is close to pi / 2
            arg.set_precision(arg.get_mantissa_max_bit_len() + ret.get_exponent().unsigned_abs() as usize, RoundingMode::None)?;

            ret = arg.cos_series(RoundingMode::None)?;
        }

        ret.set_precision(self.get_mantissa_max_bit_len(), rm)?;

        Ok(ret)
    }

    /// cosine series
    pub(super) fn cos_series(mut self, rm: RoundingMode) -> Result<Self, Error> {
        // cos:  1 - x^2/2! + x^4/4! - x^6/6! + ...

        let p = self.get_mantissa_max_bit_len();
        let mut polycoeff_gen = CosPolycoeffGen::new(p)?;
        let (reduction_times, niter) = series_cost_optimize::<CosPolycoeffGen, CosArgReductionEstimator>(
            p, &polycoeff_gen, -self.e as isize, 2, false);

        self.set_precision(p + (-COS_EXP_THRES) as usize + niter * 4 + reduction_times * 3, rm)?;

        let arg = if reduction_times > 0 {
            self.cos_arg_reduce(reduction_times, rm)?
        } else {
            self
        };

        let acc = BigFloatNumber::from_word(1, arg.get_mantissa_max_bit_len())?;  // 1
        let x_step = arg.mul(&arg, rm)?;           // x^2
        let x_first = x_step.clone()?;                 // x^2

        let ret = series_run(acc, x_first, x_step, niter, &mut polycoeff_gen, rm)?;

        if reduction_times > 0 {
            ret.cos_arg_restore(reduction_times, rm)
        } else {
            Ok(ret)
        }
    }

    // reduce argument n times.
    // cost: n * O(add)
    fn cos_arg_reduce(&self, n: usize, rm: RoundingMode) -> Result<Self, Error> {
        // cos(3*x) = - 3*cos(x) + 4*cos(x)^3
        let mut ret = self.clone()?;
        for _ in 0..n {
            ret = ret.div(&THREE, rm)?;
        }
        Ok(ret)
    }

    // restore value for the argument reduced n times.
    // cost: n * (4*O(mul) + O(add))
    fn cos_arg_restore(&self, n: usize, rm: RoundingMode) -> Result<Self, Error> {
        // cos(3*x) = - 3*cos(x) + 4*cos(x)^3
        let mut cos = self.clone()?;

        for _ in 0..n {
            let mut cos_cub = cos.mul(&cos, rm)?;
            cos_cub = cos_cub.mul(&cos, rm)?;
            let p1 = cos.mul(&THREE, rm)?;
            let p2 = cos_cub.mul(&FOUR, rm)?;
            cos = p2.sub(&p1, rm)?;
        }

        Ok(cos)
    }
}


#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_cosine() {
        let mut cc = Consts::new().unwrap();

        let rm = RoundingMode::ToEven;
        let mut n1 = BigFloatNumber::from_word(1,320).unwrap();
        n1.set_exponent(0);
        let _n2 = n1.cos(rm, &mut cc).unwrap();
        //println!("{:?}", n2.format(crate::Radix::Dec, rm).unwrap());

        // asymptotic & extrema testing
        let mut half_pi = cc.pi(128, RoundingMode::None).unwrap();
        half_pi.set_exponent(1);
        half_pi.set_precision(320, RoundingMode::None).unwrap();

        let n2 = half_pi.cos(rm, &mut cc).unwrap();
        let n3 = BigFloatNumber::parse("5.2049C1114CF98E804177D4C76273644A29410F31C6809BBDF2A33679A7486365EEEE1A43A7D13E58_e-21", crate::Radix::Hex, 640, RoundingMode::None).unwrap();

        //println!("{:?}", n2.format(crate::Radix::Bin, rm).unwrap());

        assert!(n2.cmp(&n3) == 0);

        // large exponent
        half_pi.set_exponent(256);
        let n2 = half_pi.cos(rm, &mut cc).unwrap();
        let n3 = BigFloatNumber::parse("3.2F00069261A9FFC022D09F662F2E2DDBEFD1AF138813F2A71D7601C58E793299EA052E4EBC59106C_e-1", crate::Radix::Hex, 640, RoundingMode::None).unwrap();

        //println!("{:?}", n2.format(crate::Radix::Hex, rm).unwrap());

        assert!(n2.cmp(&n3) == 0);
    }

    #[ignore]
    #[test]
    #[cfg(feature="std")]
    fn cosine_perf() {
        let mut cc = Consts::new().unwrap();

        let mut n = vec![];
        for _ in 0..10000 {
            n.push(BigFloatNumber::random_normal(133, -5, 5).unwrap());
        }

        for _ in 0..5 {
            let start_time = std::time::Instant::now();
            for ni in n.iter() {
                let _f = ni.cos(RoundingMode::ToEven, &mut cc).unwrap();
            }
            let time = start_time.elapsed();
            println!("{}", time.as_millis());
        }
    }

}