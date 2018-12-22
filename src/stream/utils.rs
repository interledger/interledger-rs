pub fn muldiv(a: u64, b: u64, c: u64) -> Result<u64, ()> {
    // Ancient Egyptian Multiplication adapted from https://bit.ly/2BAPgRB
    let mut a: u64 = a;
    let mut q: u64 = 0; // quotient
    let mut r: u64 = 0; // remainder
    let mut qn: u64 = b / c;
    let mut rn: u64 = b % c;

    while a != 0 {
        if a & 1 >= 1 {
            // TODO: map None to Error
            q = q.checked_add(qn).unwrap();
            r += rn;
            if r >= c {
                q += 1;
                r -= c;
            }
        }
        a >>= 1;
        qn <<= 1;
        rn <<= 1;
        if rn >= c {
            qn += 1;
            rn -= c;
        }
    }
    Ok(q)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod overflow {
        use super::*;

        #[test]
        fn muldiv_succeeds() {
            assert_eq!(
                muldiv(u64::max_value(), u64::max_value(), u64::max_value()).unwrap(),
                u64::max_value()
            );
        }

        #[test]
        #[should_panic]
        fn muldiv_throws() {
            muldiv(u64::max_value(), u64::max_value(), 1).unwrap();
        }
    }
}
