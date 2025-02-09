pub mod similarity;

use std::path::Path;
use std::fs::File;
use std::io::{BufWriter, Write};
use flate2::{Compression, write::GzEncoder};
use anyhow::{Result, Context};

use bed_utils::bed::{BEDLike, NarrowPeak, merge_bed_with};

pub fn merge_peaks<I>(peaks: I, half_window_size: u64) -> impl Iterator<Item = Vec<NarrowPeak>>
where
    I: Iterator<Item = NarrowPeak>,
{
    fn iterative_merge(mut peaks: Vec<NarrowPeak>) -> Vec<NarrowPeak> {
        let mut result = Vec::new();
        while !peaks.is_empty() {
            let best_peak = peaks.iter()
                .max_by(|a, b| a.p_value.partial_cmp(&b.p_value).unwrap()).unwrap()
                .clone();
            peaks = peaks.into_iter().filter(|x| x.n_overlap(&best_peak) == 0).collect();
            result.push(best_peak);
        }
        result
    }

    merge_bed_with(
        peaks.map(move |mut x| {
            let summit = x.start() + x.peak;
            x.start = summit.saturating_sub(half_window_size);
            x.end = summit + half_window_size + 1;
            x.peak = summit - x.start;
            x
        }),
        iterative_merge,
        None::<&str>,
    )
}

pub fn clip_peak(mut peak: NarrowPeak, chrom_sizes: &crate::preprocessing::count_data::ChromSizes) -> NarrowPeak {
    let new_start = peak.start().max(0);
    let new_end = peak.end().min(chrom_sizes.get(peak.chrom()).unwrap());
    peak.set_start(new_start);
    peak.set_end(new_end);
    peak.peak = (new_start + peak.peak).min(new_end) - new_start;
    peak
}

pub fn open_file_for_write<P: AsRef<Path>>(
    filename: P,
    compression: Option<&str>,
    compression_level: Option<u32>,
) -> Result<Box<dyn Write + Send>> {
    let buffer = BufWriter::new(
        File::create(&filename).with_context(|| format!("cannot create file: {}", filename.as_ref().display()))?
    );
    let writer: Box<dyn Write + Send> = match compression {
        None => Box::new(buffer),
        Some("gzip") => Box::new(GzEncoder::new(buffer, Compression::new(compression_level.unwrap_or(6)))),
        Some("zstandard") => {
            let mut zstd = zstd::stream::Encoder::new(buffer, compression_level.unwrap_or(3) as i32)?;
            zstd.multithread(8)?;
            Box::new(zstd.auto_finish())
        },
        _ => panic!("unsupported compression: {}", compression.unwrap()),
    };
    Ok(writer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bed_utils::bed::io::Reader;

    #[test]
    fn test_merge_peaks() {
        let input = "chr1\t9977\t16487\ta\t1000\t.\t74.611\t290.442\t293.049\t189
chr1\t9977\t16487\tb\t1000\t.\t74.1871\t295.33\t290.939\t425
chr1\t9977\t16487\tb\t1000\t.\t74.1871\t295\t290.939\t425
chr1\t9977\t16487\tb\t1000\t.\t74.1871\t295\t290.939\t625
chr1\t9977\t16487\tb\t1000\t.\t74.1871\t290\t290.939\t925
chr2\t9977\t16487\tb\t1000\t.\t74.1871\t295\t290.939\t625
chr2\t9977\t16487\tb\t1000\t.\t74.1871\t290\t290.939\t325
chr2\t9977\t16487\tb\t1000\t.\t74.1871\t290\t290.939\t525
chr2\t9977\t16487\tb\t1000\t.\t74.1871\t290\t290.939\t725
chr3\t0\t1164\tb\t1000\t.\t74.1871\t290\t290.939\t100
";
        let output = "chr1\t10202\t10603\tb\t1000\t.\t74.1871\t295.33\t290.939\t200
chr1\t10702\t11103\tb\t1000\t.\t74.1871\t290\t290.939\t200
chr2\t10402\t10803\tb\t1000\t.\t74.1871\t295\t290.939\t200
chr3\t0\t301\tb\t1000\t.\t74.1871\t290\t290.939\t100
";

        let expected: Vec<NarrowPeak> = Reader::new(output.as_bytes(), None)
            .into_records().map(|x| x.unwrap()).collect();
        let result: Vec<NarrowPeak> = merge_peaks(
            Reader::new(input.as_bytes(), None).into_records().map(|x| x.unwrap()),
            200
        ).flatten().collect();

        assert_eq!(expected, result);
    }
}