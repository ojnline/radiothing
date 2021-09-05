use num_traits::{Float, Num, One};
use rustfft::num_complex::Complex;

pub unsafe fn decode<T: Num + Float + Copy>(
    // the  two pointers can alias yadi yadi yada
    samples: *const Complex<T>,
    samples_len: usize,
    bits: *mut bool,
    bits_offset: usize,
    stop_bits: f32,
    baudrate: f32,
    samplerate: f32,
    letters: &mut bool,
) -> (String, *const bool, usize)
where
    Complex<T>: Num,
{
    // the formula was copied from GNU Radio quadrature demod block code
    // https://github.com/gnuradio/gnuradio/blob/1f1733eb4489b48fb73509e7806df19e1c738092/gr-analog/lib/quadrature_demod_cf_impl.cc#L42

    // I was unable to find the actual theory behind it :(

    // gr_complex* in = (gr_complex*)input_items[0];
    // float* out = (float*)output_items[0];

    // std::vector<gr_complex> tmp(noutput_items);
    // volk_32fc_x2_multiply_conjugate_32fc(&tmp[0], &in[1], &in[0], noutput_items);
    // for (int i = 0; i < noutput_items; i++) {
    //     out[i] = d_gain * gr::fast_atan2f(imag(tmp[i]), real(tmp[i]));
    // }

    // return noutput_items;

    let mut prev = Complex::<T>::one();
    let new_bits = bits.add(bits_offset);

    for i in 0..samples_len {
        let cur = *samples.add(i);

        let angle = (prev.conj() * cur).arg();

        *new_bits.add(i) = if angle.is_sign_positive() {
            true
        } else {
            false
        };

        prev = cur;
    }

    let samples_per_symbol_f = samplerate / baudrate;

    let samples_per_symbol = samples_per_symbol_f as usize;
    let half_samples_per_symbol = (samples_per_symbol_f / 2.0) as usize;
    let samples_per_stop_bit = (samples_per_symbol_f * stop_bits) as usize;

    let mut string = String::new();

    let mut cursor = bits;
    let bits_end = bits.add(samples_len);

    // the loop may exit when
    //  - bits run out while looking for a start of the char -> this is fine because calling this next time will resume exactly there
    //  - bounds checking the rest of the bits for the size of the char -> this is right after a rising edge, calling this the next
    //    time will imediatelly hit that path and end up at bounds check again, this is correct provided that the samples that were
    //    left were saved and properly restored
    //  - while looking for a falling edge after a stopbit check failed -> at this time the state is unknown anyway, it should occur
    //    only at the beggining of a transmission, TODO scan the full stopbit range for more robust checking, currently a chacter bit
    //    can be confused for a stop bit
    'decode_chars: loop {
        loop {
            if cursor == bits_end {
                break 'decode_chars;
            }

            if *cursor == true {
                break;
            }

            cursor = cursor.add(1);
        }

        let mut stopbit = cursor.add(6 * samples_per_symbol);
        if stopbit.add(samples_per_stop_bit) > bits_end {
            break 'decode_chars;
        }

        // 1 xxxxx 0..
        // ^ - found rising edge, now look at the stop bit if it really is zero, otherwise try again
        let mut stopbit_wrong = false;
        for _ in 0..samples_per_stop_bit {
            stopbit_wrong |= *stopbit == true;
            stopbit = stopbit.add(1);
        }

        if stopbit_wrong {
            cursor = cursor.add(6 * samples_per_symbol + samples_per_stop_bit);
            continue 'decode_chars;
        }
        // if *stopbit == true {
        //     // wrong
        //     // consume bits until a falling edge is found so that the rising edge loop isn't immediatelly trigerred
        //     // then restart the loop
        //     loop {
        //         if cursor == bits_end {
        //             break 'decode_chars;
        //         }

        //         if *cursor == false {
        //             continue 'decode_chars;
        //         }

        //         cursor = cursor.add(1);
        //     }
        // }

        // 1 xxxxx 0
        // --^ offset into the middle of the character pulse
        cursor = cursor.add(samples_per_symbol + half_samples_per_symbol);

        let mut char = 0;
        for i in 0..5 {
            // this cannot occur, the bounds were already checked with stopbit
            // if cursor > bits_end {
            //     break 'decode_chars;
            // }

            // 1 for true, 0 for false
            // for some reason the bits come out flipped, flip them back
            // FIXME
            let value = !*cursor as u8;

            char |= value << i;

            cursor = cursor.add(samples_per_symbol);
        }

        // TODO put this ontop the bit memory and then put it into a string all at once
        // since size_of bool == size_of u8 == 1 on any platforms that I care about
        if let Some(char) = decode_baudot(char, letters) {
            string.push(char);
        }
    }

    (string, cursor, bits_end.offset_from(cursor) as usize)
}

fn decode_baudot(bits: u8, letters: &mut bool) -> Option<char> {
    const ITA2: (&'static [u8], &'static [u8]) = (
        b"\0E\nA SIU\rDRJNFCKTZLWHYPQOBG\0MXV\0",
        b"\03\n- \x0787\r\x054',!:(5\")2#6019?&\0./;\0",
    );

    match (bits, *letters) {
        (0b11111u8, _) => {
            *letters = true;
            None
        }
        (0b11011u8, _) => {
            *letters = false;
            None
        }
        (letter, true) => Some(ITA2.0[letter as usize] as char),
        (figure, false) => Some(ITA2.1[figure as usize] as char),
    }
}
