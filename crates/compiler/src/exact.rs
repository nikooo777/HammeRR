use std::cmp::Ordering;

use bnum::{cast::As, n, types::I1024};

const MAX_DYADIC_POWER: i32 = I1024::BITS as i32 - 2;
pub(crate) const ARITHMETIC_ERROR: &str =
    "Exact geometry exceeds checked 1023-bit rational arithmetic";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rational {
    pub(crate) numerator: I1024,
    pub(crate) denominator: I1024,
}

pub(crate) fn gcd(mut a: I1024, mut b: I1024) -> I1024 {
    while !b.is_zero() {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

impl Rational {
    pub(crate) const ZERO: Self = Self {
        numerator: n!(0),
        denominator: n!(1),
    };
    pub(crate) const ONE: Self = Self {
        numerator: n!(1),
        denominator: n!(1),
    };

    pub(crate) fn new(numerator: i128, denominator: i128) -> Result<Self, String> {
        let numerator = numerator.as_::<I1024>();
        let denominator = denominator.as_::<I1024>();
        Self::ratio(numerator, denominator)
    }

    pub(crate) fn ratio(mut numerator: I1024, mut denominator: I1024) -> Result<Self, String> {
        if denominator.is_zero() || numerator == I1024::MIN || denominator == I1024::MIN {
            return Err(ARITHMETIC_ERROR.into());
        }
        if denominator.is_negative() {
            numerator = -numerator;
            denominator = -denominator;
        }
        let divisor = gcd(numerator.abs(), denominator);
        Ok(Self {
            numerator: numerator / divisor,
            denominator: denominator / divisor,
        })
    }

    pub(crate) fn from_f64(value: f64) -> Result<Self, String> {
        if !value.is_finite() {
            return Err("Exact geometry requires finite prepared coordinates".into());
        }
        if value == 0.0 {
            return Ok(Self::ZERO);
        }
        let bits = value.to_bits();
        let exponent = ((bits >> 52) & 0x7ff) as i32;
        let mut mantissa = bits & ((1u64 << 52) - 1);
        let mut power = if exponent == 0 {
            -1074
        } else {
            exponent - 1075
        };
        if exponent != 0 {
            mantissa |= 1u64 << 52;
        }
        let trailing = mantissa.trailing_zeros();
        mantissa >>= trailing;
        power += trailing as i32;
        let mut numerator = mantissa.as_::<I1024>();
        let denominator = if power >= 0 {
            if power > MAX_DYADIC_POWER || numerator > (I1024::MAX >> power) {
                return Err(ARITHMETIC_ERROR.into());
            }
            numerator <<= power;
            n!(1)
        } else {
            if power < -MAX_DYADIC_POWER {
                return Err(ARITHMETIC_ERROR.into());
            }
            n!(1I1024) << -power
        };
        if bits >> 63 != 0 {
            numerator = -numerator;
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }

    pub(crate) fn sign(self) -> Ordering {
        Ord::cmp(&self.numerator, &n!(0))
    }

    pub(crate) fn neg(self) -> Self {
        Self {
            numerator: -self.numerator,
            denominator: self.denominator,
        }
    }

    pub(crate) fn abs(self) -> Self {
        Self {
            numerator: self.numerator.abs(),
            denominator: self.denominator,
        }
    }

    pub(crate) fn add(self, other: Self) -> Result<Self, String> {
        let divisor = gcd(self.denominator, other.denominator);
        let left = self
            .numerator
            .checked_mul(other.denominator / divisor)
            .ok_or(ARITHMETIC_ERROR)?;
        let right = other
            .numerator
            .checked_mul(self.denominator / divisor)
            .ok_or(ARITHMETIC_ERROR)?;
        let numerator = left.checked_add(right).ok_or(ARITHMETIC_ERROR)?;
        if numerator == I1024::MIN {
            return Err(ARITHMETIC_ERROR.into());
        }
        let reduction = gcd(numerator.abs(), divisor);
        let denominator = (self.denominator / divisor)
            .checked_mul(other.denominator / reduction)
            .ok_or(ARITHMETIC_ERROR)?;
        Ok(Self {
            numerator: numerator / reduction,
            denominator,
        })
    }

    pub(crate) fn sub(self, other: Self) -> Result<Self, String> {
        self.add(other.neg())
    }

    pub(crate) fn mul(self, other: Self) -> Result<Self, String> {
        let left = gcd(self.numerator.abs(), other.denominator);
        let right = gcd(other.numerator.abs(), self.denominator);
        let numerator = (self.numerator / left)
            .checked_mul(other.numerator / right)
            .ok_or(ARITHMETIC_ERROR)?;
        let denominator = (self.denominator / right)
            .checked_mul(other.denominator / left)
            .ok_or(ARITHMETIC_ERROR)?;
        if numerator == I1024::MIN {
            return Err(ARITHMETIC_ERROR.into());
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }

    pub(crate) fn div(self, other: Self) -> Result<Self, String> {
        if other.numerator.is_zero() {
            return Err("Exact geometry encountered a zero rational denominator".into());
        }
        let inverse = Self {
            numerator: if other.numerator.is_negative() {
                -other.denominator
            } else {
                other.denominator
            },
            denominator: other.numerator.abs(),
        };
        self.mul(inverse)
    }

    pub(crate) fn compare(self, other: Self) -> Ordering {
        let sign = self.sign().cmp(&other.sign());
        if sign != Ordering::Equal {
            return sign;
        }
        let mut a = self.numerator.abs();
        let mut b = self.denominator;
        let mut c = other.numerator.abs();
        let mut d = other.denominator;
        let mut reverse = self.numerator.is_negative();
        loop {
            let order = Ord::cmp(&(a / b), &(c / d));
            if order != Ordering::Equal {
                return if reverse { order.reverse() } else { order };
            }
            let left = a % b;
            let right = c % d;
            if left.is_zero() || right.is_zero() {
                let order = Ord::cmp(&left, &right);
                return if reverse { order.reverse() } else { order };
            }
            (a, b, c, d) = (b, left, d, right);
            reverse = !reverse;
        }
    }

    pub(crate) fn approximate(self) -> f64 {
        self.numerator.as_::<f64>() / self.denominator.as_::<f64>()
    }

    pub(crate) fn upper(self) -> Result<f64, String> {
        let mut value = self.approximate();
        for _ in 0..8 {
            if Self::from_f64(value)?.compare(self) != Ordering::Less {
                return Ok(value);
            }
            value = value.next_up();
        }
        Err("Exact geometry cannot bound binary64 conversion".into())
    }

    pub(crate) fn lower(self) -> Result<f64, String> {
        let mut value = self.approximate();
        for _ in 0..8 {
            if Self::from_f64(value)?.compare(self) != Ordering::Greater {
                return Ok(value);
            }
            value = value.next_down();
        }
        Err("Exact geometry cannot bound binary64 conversion".into())
    }
}
