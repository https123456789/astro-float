use astro_float_num::BigFloat;
use astro_float_num::Consts;
use astro_float_num::RoundingMode;
use astro_float_macro::expr;

fn main() {
    let rm = RoundingMode::None;
    let mut cc = Consts::new().unwrap();
    let _res: BigFloat = expr!(-6 * atan(1.0 / sqrt(3)), (256, rm, &mut cc));
}
