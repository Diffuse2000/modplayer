use crate::envelope::{Envelope, EnvelopePoint};
use crate::tables;
use crate::tables::{LINEAR_PERIODS, AMIGA_PERIODS};
use std::sync::atomic::Ordering::{Acquire, Relaxed};
use crate::channel_state::channel_state::TableType::AmigaFrequency;
use std::num::Wrapping;

pub static mut USE_AMIGA : std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);


/// A value bounded by a minimum and a maximum
///
///  If input is less than min then this returns min.
///  If input is greater than max then this returns max.
///  Otherwise this returns input.
///
/// **Panics** in debug mode if `!(min <= max)`.
#[inline]
pub fn clamp<T: PartialOrd>(input: T, min: T, max: T) -> T {
    debug_assert!(min <= max, "min must be less than or equal to max");
    if input < min {
        min
    } else if input > max {
        max
    } else {
        input
    }
}

fn log(i: usize) -> f64 {
    (i as f64 / 768.0).exp2() * (8363.0 * 256.0)
}

fn log_table() -> [f64; 768] {
    let mut result = [0.0f64; 768];
    for i in 0..768 {
        result[i] = log(i);
    }

    result
}

lazy_static! {
static ref D_LOG_TAB: [f64; 768] = log_table();
}

#[derive(Eq, PartialEq)]
enum TableType {
    LinearFrequency,
    AmigaFrequency
}

struct AudioTables {
    Periods:        [i16; 1936],
    dPeriod2HzTab:  [f64; 65536],
}

impl AudioTables {
    fn calcTablesLinear() -> Self // taken directly from ft2clone
    {
        let mut result = Self { Periods: [0i16; 1936], dPeriod2HzTab: [0.0f64; 65536] };
        result.dPeriod2HzTab[0] = 0.0; // in FT2, a period of 0 yields 0Hz

        result.Periods = LINEAR_PERIODS;
        // linear periods
        for i in 1..65536 {
            let invPeriod = (12 * 192 * 4 as u16).wrapping_sub(i as u16); // this intentionally overflows uint16_t to be accurate to FT2
            let octave = invPeriod as u32 / 768;
            let period = invPeriod as u32 % 768;
            let bitshift = (14u32.wrapping_sub(octave)) & 0x1F; // 100% accurate to FT2

            result.dPeriod2HzTab[i] = D_LOG_TAB[period as usize] / (1 << bitshift) as f64;
        }
        result
    }

    fn calcTablesAmiga() -> AudioTables // taken directly from ft2clone
    {
        let mut result = Self { Periods: [0i16; 1936], dPeriod2HzTab: [0.0f64; 65536] };
        result.dPeriod2HzTab[0] = 0.0; // in FT2, a period of 0 yields 0Hz
        result.Periods = AMIGA_PERIODS;
        // Amiga periods
        for i in 1..65536 {
            result.dPeriod2HzTab[i] = (8363.0 * 1712.0) / i as f64;
        }
        result
    }
}

lazy_static! {
static ref AmigaTables:   AudioTables   = AudioTables::calcTablesAmiga();
static ref LinearTables:  AudioTables   = AudioTables::calcTablesLinear();
}

#[derive(Clone,Copy,Debug)]
pub(crate) struct PortaToNoteState {
    pub(crate) target_note:                Note,
    pub(crate) speed:                      u8,
}


impl PortaToNoteState {
    pub(crate) fn new() -> PortaToNoteState {
        PortaToNoteState {
            target_note: Note{
                note: 0,
                finetune: 0,
                period: 0
            },
            speed: 0
        }
    }
}

pub(crate) enum WaveControl {
    SIN,
    RAMP,
    SQUARE,
}

const SIN_TABLE: [i32; 32] =
    [0,   24,   49,  74,  97, 120, 141, 161,
        180, 197, 212, 224, 235, 244, 250, 253,
        255, 253, 250, 244, 235, 224, 212, 197,
        180, 161, 141, 120,  97,  74,  49,  24];

#[derive(Clone,Copy,Debug)]
pub(crate) struct VibratoState {
    speed:  i8,
    depth:  i8,
    pos:    i8,
}

impl VibratoState {

    pub(crate) fn new() -> VibratoState {
        VibratoState {
            speed: 0,
            depth: 0,
            pos: 0
        }
    }

    pub(crate) fn set_speed(&mut self, speed: i8) {
        if speed != 0 {
            self.speed = speed;
        }
    }

    pub(crate) fn set_depth(&mut self, depth: i8) {
        if depth != 0 {
            self.depth = depth;
        }
    }

    pub(crate) fn get_frequency_shift(&mut self, wave_control: WaveControl) -> i32 {
        let delta;
        let vibrato_pos = (self.pos >> 2) & 31;
        match wave_control {
            WaveControl::SIN => { delta = SIN_TABLE[vibrato_pos as usize] * self.depth as i32; }
            WaveControl::RAMP => {
                let temp:i32 = (vibrato_pos * 8) as i32;
                delta = if self.pos < 0 { 255 - temp } else { temp } as i32
            }
            WaveControl::SQUARE => { delta = 255; }
        }
        ((delta >> 5) * (if self.pos < 0 { -1 } else { 1 })) as i32
    }

    pub(crate) fn next_tick(&mut self) {
        self.pos += self.speed;
        if self.pos > 31 { self.pos -= 64; }
    }
}

// This and vibrato should derive from a base class probably
#[derive(Clone,Copy,Debug)]
pub(crate) struct TremoloState {
    speed:  i8,
    depth:  i8,
    pos:    i8,
}

impl TremoloState {

    pub(crate) fn new() -> TremoloState {
        TremoloState {
            speed: 0,
            depth: 0,
            pos: 0
        }
    }

    pub(crate) fn set_speed(&mut self, speed: i8) {
        if speed != 0 {
            self.speed = speed;
        }
    }

    pub(crate) fn set_depth(&mut self, depth: i8) {
        if depth != 0 {
            self.depth = depth;
        }
    }


    pub(crate) fn get_volume_shift(&mut self, wave_control: WaveControl) -> i32 {
        let delta;
        let termolo_pos = (self.pos >> 2) & 31;
        match wave_control {
            WaveControl::SIN => { delta = SIN_TABLE[termolo_pos as usize] as i32; }
            WaveControl::RAMP => {
                let temp:i32 = (termolo_pos * 8) as i32;
                delta = if self.pos < 0 { 255 - temp } else { temp } as i32
            }
            WaveControl::SQUARE => { delta = 255; }
        }
        (((delta * self.depth as i32) >> 6) * (if self.pos < 0 { -1 } else { 1 })) as i32
    }

    pub(crate) fn next_tick(&mut self) {
        self.pos += self.speed;
        if self.pos > 31 { self.pos -= 64; }
    }
}


#[derive(Clone,Copy,Debug)]
pub(crate) struct EnvelopeState {
    frame:      u16,
    pub(crate) sustained:  bool,
    looped:     bool,
    idx:        usize,
    // instrument:         &'a Instrument,
}

impl EnvelopeState {
    pub(crate) fn new() -> EnvelopeState {
        EnvelopeState { frame: 0, sustained: false, looped: false, idx: 0 }
    }

    pub(crate) fn handle(&mut self, env: &Envelope, channel_sustained: bool, default: u16, sticky_sustain: bool) -> u16 {
        if !env.on || env.size < 1 { return default * 256;} // bail out

        if env.size == 1 { // whatever
            return env.points[0].value * 256
        }

        // set sustained if channel_state is sustained, we have a sustain point and we reached the sustain point
        // I did all my testing on the panning envelope, and caught this,
        // but later realized that volume envelopes works differently, and "fixed it". Oh, Well... xm...
        if !self.sustained && env.sustain && channel_sustained && self.frame == env.points[env.sustain_point as usize].frame {
            self.sustained = true;
        }

        // if sustain was triggered, it's sticky if panning envelope
        if self.sustained && (channel_sustained || sticky_sustain) {
            return env.points[env.sustain_point as usize].value * 256
        }

        // loop
        if env.has_loop && self.frame == env.points[env.loop_end_point as usize].frame {
            self.looped = true;
            self.idx = env.loop_start_point as usize;
            self.frame = env.points[self.idx].frame;
        }

        // reached the end
        if self.idx == (env.size - 2) as usize && self.frame == env.points[self.idx + 1].frame {
            return env.points[self.idx + 1].value * 256
        }

        let retval = EnvelopeState::lerp(self.frame, &env.points[self.idx], &env.points[self.idx + 1]);

        if self.frame < env.points[(env.size - 1) as usize].frame {
            self.frame += 1;
            if self.idx < (env.size - 2) as usize && self.frame == env.points[self.idx + 1].frame {
                self.idx += 1;
            }
        }

        retval
    }

    pub(crate) fn set_position(&mut self, env: &Envelope, pos: u8) {
        // // pre: envelope exists and should be set
        //
        // if env.size < 2 {
        //     return;
        // }
        //
        // let mut idx = 0;
        // while idx <

    }

        // pre: e.on && e.size > 0
//    default: panning envelope: middle (0x80?), volume envelope - max (0x40)
//     pub fn handle1(&mut self, e: &Envelope, sustained: bool, default: u16) -> u16 {
//         // fn handle(&mut self, e: &Envelope, channel_sustained: bool) -> u16 {
//         if !e.on || e.size < 1 { return default * 256;} // bail out
//         if e.size == 1 {
//             // if !e.sustain {return default;}
//             return e.points[0].value * 256;
//         }
//
//         if e.has_loop && self.frame >= e.points[e.loop_end_point as usize].frame as u32 as u16 {
//             self.frame = e.points[e.loop_start_point as usize].frame as u32 as u16
//         }
//
//         let mut idx:usize = 0;
//         loop {
//             if idx >= e.size as usize - 2 { break; }
//             if e.points[idx].frame as u32 <= self.frame as u32 && e.points[idx+1].frame as u32 >= self.frame as u32 {
//                 break;
//             }
//             idx += 1;
//         }
//
//         // if sustained && (e.sustain && self.idx == e.sustain_point as u32) && self.idx == e.size as u32 {
//         //     return e.points[self.idx as usize].value;
//         // }
//
//         let retval = EnvelopeState::lerp(self.frame as u16, &e.points[idx as usize], &e.points[(idx + 1) as usize]);
//
//
//         if !sustained || !e.sustain || self.frame != e.points[e.sustain_point as usize].frame as u32 as u16 {
//             self.frame += 1;
//         }
//
//         retval
//     }

    fn lerp(frame: u16, e1: &EnvelopePoint, e2: &EnvelopePoint) -> u16 {
        if frame == e1.frame {
            return e1.value * 256;
        } else if frame == e2.frame {
            return e2.value * 256;
        }

        let t = (frame - e1.frame) as f32 / (e2.frame - e1.frame) as f32;

        return clamp((((1.0 - t) * e1.value as f32 + t * e2.value as f32) * 256.0) as i32, 0, 65535) as u16;
    }

    // fn next_tick(& mut self) {
    //     if self.instrument.volume_points > 0 {
    //         self.volume_frame += 1;
    //         if self.volume_frame >= self.instrument.volume_envelope[self.volume_idx + 1].frame_number {
    //             self.volume_idx +=1;
    //             if self.volume_idx > self.instrument.volume_loop_end_point as u32 {
    //                 self.volume_idx = self.instrument.volume_loop_start_point as u32;
    //                 self.volume_frame = self.instrument.volume_envelope[self.volume_idx];
    //
    //             }
    //
    //         }
    //     }
    //     // if self.volume_frame > self.instrument.panning_loop_end_point {
    //     //
    //     // }
    //     // self.panning_frame  += 1;
    // }

    pub(crate) fn reset(& mut self, pos: u16, env: &Envelope) {
        if !env.on {return;}
        self.frame      = pos;
        self.sustained  = false;
        self.looped     = false;

        if self.frame > env.points[(env.size - 1) as usize].frame {self.frame = env.points[(env.size - 1) as usize].frame;}
        let mut idx:usize = 0;
        loop {
            if idx >= env.size as usize - 2 { break; }
            if env.points[idx].frame <= self.frame && env.points[idx+1].frame >= self.frame {
                break;
            }
            idx += 1;
        }
        self.idx = idx;
    }
}

#[derive(Clone,Copy,Debug)]
pub(crate) struct Note {
    note:       u8,
    finetune:   i8,
    pub(crate) period:     i16
}

impl Note {

    pub(crate) fn new() -> Note {
        Note{
            note: 0,
            finetune: 0,
            period: 0
        }
    }

    // note <= 120
    pub(crate) fn set_note(&mut self, note: u8, finetune: i8) {

        self.note = note;
        self.finetune = finetune;
        let sidx= (self.note as i32 - 1) * 16 + ((self.finetune >> 3) + 16) as i32;
        let idx = clamp(sidx, 0, 1935);

        unsafe { self.period = if USE_AMIGA.load(Relaxed) { tables::AMIGA_PERIODS[idx as usize] } else { tables::LINEAR_PERIODS[idx as usize] }; }
    }


    pub(crate) fn get_tone(note: u8, relative_note: i8) -> Result<u8, bool> {
        let tone = note as i8 + relative_note;
        if tone > 12 * 10 || tone < 0 {
            return Err(false);
        }
        Ok(tone as u8)
    }


    // indexing:   let idx = (self.note - 1) as u32 * 16 + ((self.finetune >> 3) + 16) as u32;
    // note is 7 bits << 4  - bits 11..4
    // finetune is 8 bit >> 3 = 5 bits - 0..31 (after fixup)

    //  B-3 -15 -14 -13 -12 -11 -10 -9 -8 -9 -6 -5 -4 -3 -2 -1 C-4 +1 +2 +3 +4 +5 +6 +7 +8 +9 +10 +11 +12 +13 +14 +15 D-4
    //  -16
    //  note: C-4
    //  C-4 - 1 = B-3
    //  FineTune +16 shifts the the FineTune into place. We subtract 1 * 16 from note, and fixup the FineTune
    //  Which means we can just do a binary search inside the table and round to the nearest semi tone...
    fn nearestSemiTone(&self, period: u16, added_note: u8) -> i16 {

        let use_amiga: bool = unsafe { USE_AMIGA.load(Acquire) };
        let mut note2period: &[i16; 1936];
        if use_amiga {
            note2period = &AMIGA_PERIODS;
        } else {
            note2period = &LINEAR_PERIODS;
        }

        let needed_period = period as i16;
        let idx = (note2period.binary_search_by(|element| needed_period.cmp(element)).unwrap_or_else(|x| x)) & !0xf;

        let mut fixed_note_id = idx as isize + (self.finetune / 8) as isize;// + (added_note as isize * 16);

        if fixed_note_id < 16 {fixed_note_id = 16}
        if fixed_note_id > (8*12*16) {fixed_note_id = (8*12*16)}

        let clamped_note_idx = clamp(fixed_note_id, 0, 1935) as usize;
        note2period[clamped_note_idx]
    }

    // for arpeggio and portamento (semitone-slide mode). Lifted directly from ft2-clone. I'll have to write it from scratch one day
    fn relocateTon(&self, period: u16, arpNote: u8) -> i16 {
        // int32_t fineTune, loPeriod, hiPeriod, tmpPeriod, tableIndex;
        let fineTune : i32 = (((self.finetune >> 3) + 16) << 1) as i32;
        let mut hiPeriod : u32 = (8 * 12 * 16) * 2;
        let mut loPeriod : u32 = 0;

        let use_amiga: bool = unsafe { USE_AMIGA.load(Acquire) };
        let mut note2Period: &[i16; 1936];
        if use_amiga {
            note2Period = &AMIGA_PERIODS;
        } else {
            note2Period = &LINEAR_PERIODS;
        }

        for i in 0..8 {
            let tmpPeriod = (((loPeriod + hiPeriod) >> 1) & 0xFFFFFFE0) as i32 + fineTune;

            let mut tableIndex = (tmpPeriod - 16) as i32 >> 1;
            tableIndex = clamp(tableIndex, 0, 1935); // 8bitbubsy: added security check

            if period >= note2Period[tableIndex as usize] as u16 {
                hiPeriod = (tmpPeriod - fineTune) as u32 & 0xFFFFFFE0;
            } else {
                loPeriod = (tmpPeriod - fineTune) as u32 & 0xFFFFFFE0;
            }
        }

        // arpNote is between 0..16
        let mut tmpPeriod = loPeriod + fineTune as u32 + (arpNote << 5) as u32;

        if tmpPeriod < 0 {// 8bitbubsy: added security check
            tmpPeriod = 0;
        }

        if tmpPeriod >= (8*12*16+15)*2-1 { // FT2 bug: off-by-one edge case
            tmpPeriod = (8 * 12 * 16 + 15) * 2;
        }

        return note2Period[(tmpPeriod>>1) as usize];
    }



    pub(crate) fn frequency(&self, period_shift: i16, semitone: bool) -> f32 {
        // let period = 10.0 * 12.0 * 16.0 * 4.0 - ((self.note - period_shift) * 16.0 * 4.0)  - self.finetune / 2.0;
        // if semitone {
        let mut period:i16 = 0;
        if semitone {
            period = self.relocateTon(self.period as u16, period_shift as u8);
        } else {
            period = (self.period as i16 - (period_shift * 16 * 4) as i16) ;
        }
        if period < 0 {
            println!("{}, {}", self.period, period_shift);
        }
        // }

        period = clamp(period, 0, 31999);
        let mut use_amiga : bool;
        unsafe { use_amiga = USE_AMIGA.load(Acquire); }
        return if use_amiga {
            AmigaTables.dPeriod2HzTab[period as usize] as f32
        } else {
            LinearTables.dPeriod2HzTab[period as usize] as f32
        }
        // let two = 2.0f32;
        // let freq = 8363.0 * two.powf((6 * 12 * 16 * 4 - period) as f32 / (12 * 16 * 4) as f32);
        // return freq
    }

    const NOTES: [&'static str;12] = ["C-", "C#", "D-", "D#", "E-", "F-", "F#", "G-", "G#", "A-", "A#", "B-"];

    pub(crate) fn to_string(&self) -> String {
        if self.note == 97 || self.note == 0 { (self.note as u32).to_string() } else {
            format!("{}{}", Self::NOTES[((self.note as u8 - 1) % 12) as usize], (((self.note as u8 - 1) / 12) + '0' as u8) as char )
        }
    }
}


#[cfg(test)]
mod tests {
    use crate::channel_state::channel_state::Note;

    #[test]
    fn it_works() {
        for note_idx in 0..121 {
            for finetune in -128..=127 {
                for added_note in 0..1 {
                    let mut note = Note::new();
                    note.set_note(note_idx, finetune);

                    let actual = note.nearestSemiTone(note.period as u16, added_note);
                    let expected = note.relocateTon(note.period as u16, added_note);
                    println!("expected: {}, actual: {}, note {:3}, finetune {:3}, added_note:{:3}, {}", expected, actual, note_idx, finetune, added_note, expected != actual);
                    // assert_eq!(actual, expected, "note: {}, finetune: {}, added_note: {}", note_idx, finetune, added_note)
                    if note_idx == 119 {assert!(false, "blah")}
                }
            }
        }
    }
}


#[derive(Clone,Copy,Debug)]
pub(crate) struct Volume {
    pub(crate) volume:         u8,
    pub(crate) volume_shift:   i32,
    pub(crate) output_volume:  f32,
    pub(crate) fadeout_vol:    i32,
    pub(crate) fadeout_speed:  i32,
    pub(crate) envelope_vol:   i32,
    pub(crate) global_vol:     i32,
}

impl Volume {
    pub(crate) fn new() -> Volume {
        Volume {
            volume: 0,
            volume_shift: 0,
            output_volume: 1.0,
            fadeout_vol: 65536,
            fadeout_speed: 0,
            envelope_vol: 0,
            global_vol: 0
        }
    }

    pub(crate) fn get_volume(&self) -> u8 {
        let outvol = self.volume as i32 + self.volume_shift;
        if outvol > 64 {64} else if outvol < 0 {0} else {outvol as u8}
    }

    pub(crate) fn retrig(&mut self, vol: i32) {
        self.set_volume(vol);
        self.volume_shift  = 0;
        self.fadeout_speed = 0;
        self.fadeout_vol = 65536;
        self.fadeout_speed = 0;
    }

    pub(crate) fn set_volume(&mut self, vol: i32) {
        self.volume = if vol > 64 { 64 } else if vol < 0 { 0 } else { vol } as u8;
    }
}


#[derive(Clone,Copy,Debug)]
pub(crate) struct Panning {
    pub(crate) panning:               u8,
    pub(crate) final_panning:         u8,
}

impl Panning {
    pub(crate) fn new() -> Panning {
        Panning {
            panning: 0x80,
            final_panning: 128,
        }
    }

    pub(crate) fn get_panning(&self) -> u8 {
        // let outvol = self.volume as i32 + self.volume_shift;
        // if outvol > 64 {64} else if outvol < 0 {0} else {outvol as u8}
        0x80
    }

    pub(crate) fn set_panning(&mut self, panning: i32) {
        self.panning = clamp(panning , 0, 255) as u8;
    }

    pub(crate) fn update_envelope_panning(&mut self, envelope_panning: u16) {
        self.final_panning = clamp(self.panning as i32 + (envelope_panning as i32-32*256)*(128 - (self.panning as i32 - 128).abs()) / (32i32 * 128i32), 0 ,255) as u8;
        // self.panning = clamp(self.panning , 0, 255) as u8;
    }
}
