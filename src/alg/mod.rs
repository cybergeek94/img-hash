#![allow(clippy::needless_lifetimes)]
use crate::CowImage::*;
use crate::HashVals::*;
use crate::{BitSet, HashCtxt, Image};

use self::HashAlg::*;

mod blockhash;

/// Hash algorithms implemented by this crate.
///
/// Implemented primarily based on the high-level descriptions on the blog Hacker Factor
/// written by Dr. Neal Krawetz: http://www.hackerfactor.com/
///
/// Note that `hash_width` and `hash_height` in these docs refer to the parameters of
/// [`HasherConfig::hash_size()`](struct.HasherConfig.html#method.hash_size).
///
/// ### Choosing an Algorithm
/// Each algorithm has different performance characteristics
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HashAlg {
    /// The Mean hashing algorithm.
    ///
    /// The image is converted to grayscale, scaled down to `hash_width x hash_height`,
    /// the mean pixel value is taken, and then the hash bits are generated by comparing
    /// the pixels of the descaled image to the mean.
    ///
    /// This is the most basic hash algorithm supported, resistant only to changes in
    /// resolution, aspect ratio, and overall brightness.
    ///
    /// Further Reading:
    /// http://www.hackerfactor.com/blog/?/archives/432-Looks-Like-It.html
    Mean,

    /// The Median hashing algorithm.
    ///
    /// The image is converted to grayscale, scaled down to `hash_width x hash_height`,
    /// the median pixel value is taken, and then the hash bits are generated by comparing
    /// the pixels of the descaled image to the mean.
    ///
    /// Median hashing in combiantion with preproc_dct is the basis for pHash
    Median,

    /// The Gradient hashing algorithm.
    ///
    /// The image is converted to grayscale, scaled down to `(hash_width + 1) x hash_height`,
    /// and then in row-major order the pixels are compared with each other, setting bits
    /// in the hash for each comparison. The extra pixel is needed to have `hash_width` comparisons
    /// per row.
    ///
    /// This hash algorithm is as fast or faster than Mean (because it only traverses the
    /// hash data once) and is more resistant to changes than Mean.
    ///
    /// Further Reading:
    /// http://www.hackerfactor.com/blog/index.php?/archives/529-Kind-of-Like-That.html
    Gradient,

    /// The Vertical-Gradient hashing algorithm.
    ///
    /// Equivalent to [`Gradient`](#variant.Gradient) but operating on the columns of the image
    /// instead of the rows.
    VertGradient,

    /// The Double-Gradient hashing algorithm.
    ///
    /// An advanced version of [`Gradient`](#variant.Gradient);
    /// resizes the grayscaled image to `(width / 2 + 1) x (height / 2 + 1)` and compares columns
    /// in addition to rows.
    ///
    /// This algorithm is slightly slower than `Gradient` (resizing the image dwarfs
    /// the hash time in most cases) but the extra comparison direction may improve results (though
    /// you might want to consider increasing
    /// [`hash_size`](struct.HasherConfig.html#method.hash_size)
    /// to accommodate the extra comparisons).
    DoubleGradient,

    /// The [Blockhash.io](https://blockhash.io) algorithm.
    ///
    /// Compared to the other algorithms, this does not require any preprocessing steps and so
    /// may be significantly faster at the cost of some resilience.
    ///
    /// The algorithm is described in a high level here:
    /// https://github.com/commonsmachinery/blockhash-rfc/blob/master/main.md
    Blockhash,
}

/// The bit order used when forming the bit string of the hash
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize, Clone, Copy)]
pub enum BitOrder {
    /// Least Significant Bit First. This turns a filter output of 1000 000 into the hash 0x01
    ///
    /// This is the traditional mode of this library
    LsbFirst,

    /// Most Significant Bit First. This turns a filter output of 1000 000 into the hash 0x80
    ///
    /// This mode is popular among other libraries, and thus useful to generate hashes compatible with them
    MsbFirst,
}

fn next_multiple_of_2(x: u32) -> u32 {
    (x + 1) & !1
}

fn next_multiple_of_4(x: u32) -> u32 {
    (x + 3) & !3
}

impl HashAlg {
    pub(crate) fn hash_image<I, B>(&self, ctxt: &HashCtxt, image: &I) -> B
    where
        I: Image,
        B: BitSet,
    {
        let post_gauss = ctxt.gauss_preproc(image);

        let HashCtxt {
            width,
            height,
            bit_order,
            ..
        } = *ctxt;

        if *self == Blockhash {
            return match post_gauss {
                Borrowed(img) => blockhash::blockhash(img, width, height, bit_order),
                Owned(img) => blockhash::blockhash(&img, width, height, bit_order),
            };
        }

        let grayscale = post_gauss.to_grayscale();
        let (resize_width, resize_height) = self.resize_dimensions(width, height);

        let hash_vals = ctxt.calc_hash_vals(&grayscale, resize_width, resize_height);

        let rowstride = resize_width as usize;

        match (*self, hash_vals) {
            (Mean, Floats(ref floats)) => B::from_bools(mean_hash_f32(floats), bit_order),
            (Mean, Bytes(ref bytes)) => B::from_bools(mean_hash_u8(bytes), bit_order),
            (Gradient, Floats(ref floats)) => {
                B::from_bools(gradient_hash(floats, rowstride), bit_order)
            }
            (Gradient, Bytes(ref bytes)) => {
                B::from_bools(gradient_hash(bytes, rowstride), bit_order)
            }
            (VertGradient, Floats(ref floats)) => {
                B::from_bools(vert_gradient_hash(floats, rowstride), bit_order)
            }
            (VertGradient, Bytes(ref bytes)) => {
                B::from_bools(vert_gradient_hash(bytes, rowstride), bit_order)
            }
            (DoubleGradient, Floats(ref floats)) => {
                B::from_bools(double_gradient_hash(floats, rowstride), bit_order)
            }
            (DoubleGradient, Bytes(ref bytes)) => {
                B::from_bools(double_gradient_hash(bytes, rowstride), bit_order)
            }
            (Median, Floats(ref floats)) => B::from_bools(median_hash_f32(floats), bit_order),
            (Median, Bytes(ref bytes)) => B::from_bools(median_hash_u8(bytes), bit_order),
            (Blockhash, _) => unreachable!(),
        }
    }

    pub(crate) fn round_hash_size(&self, width: u32, height: u32) -> (u32, u32) {
        match *self {
            DoubleGradient => (next_multiple_of_2(width), next_multiple_of_2(height)),
            Blockhash => (next_multiple_of_4(width), next_multiple_of_4(height)),
            _ => (width, height),
        }
    }

    pub(crate) fn resize_dimensions(&self, width: u32, height: u32) -> (u32, u32) {
        match *self {
            Mean => (width, height),
            Median => (width, height),
            Blockhash => panic!("Blockhash algorithm does not resize"),
            Gradient => (width + 1, height),
            VertGradient => (width, height + 1),
            DoubleGradient => (width / 2 + 1, height / 2 + 1),
        }
    }
}

fn mean_hash_u8<'a>(luma: &'a [u8]) -> impl Iterator<Item = bool> + 'a {
    let mean = (luma.iter().map(|&l| l as u32).sum::<u32>() / luma.len() as u32) as u8;
    luma.iter().map(move |&x| x >= mean)
}

fn mean_hash_f32<'a>(luma: &'a [f32]) -> impl Iterator<Item = bool> + 'a {
    let mean = luma.iter().sum::<f32>() / luma.len() as f32;
    luma.iter().map(move |&x| x >= mean)
}

fn median_f32(numbers: &[f32]) -> f32 {
    let mut sorted = numbers.to_owned();
    sorted.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());

    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        let a = sorted[mid - 1];
        let b = sorted[mid];
        (a + b) / 2.0
    } else {
        sorted[mid]
    }
}

fn median_u8(numbers: &[u8]) -> u8 {
    let mut sorted = numbers.to_owned();
    sorted.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());

    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        let a = sorted[mid - 1];
        let b = sorted[mid];
        ((a as u16 + b as u16) / 2) as u8
    } else {
        sorted[mid]
    }
}

fn median_hash_u8<'a>(luma: &'a [u8]) -> impl Iterator<Item = bool> + 'a {
    let med = median_u8(luma);
    luma.iter().map(move |&x| x >= med)
}

fn median_hash_f32<'a>(luma: &'a [f32]) -> impl Iterator<Item = bool> + 'a {
    let med = median_f32(luma);
    luma.iter().map(move |&x| x >= med)
}

/// The guts of the gradient hash separated so we can reuse them
fn gradient_hash_impl<I>(luma: I) -> impl Iterator<Item = bool>
where
    I: IntoIterator + Clone,
    <I as IntoIterator>::Item: PartialOrd,
{
    luma.clone()
        .into_iter()
        .skip(1)
        .zip(luma)
        .map(|(this, last)| last < this)
}

fn gradient_hash<'a, T: PartialOrd>(
    luma: &'a [T],
    rowstride: usize,
) -> impl Iterator<Item = bool> + 'a {
    luma.chunks(rowstride).flat_map(gradient_hash_impl)
}

fn vert_gradient_hash<'a, T: PartialOrd>(
    luma: &'a [T],
    rowstride: usize,
) -> impl Iterator<Item = bool> + 'a {
    (0..rowstride)
        .map(move |col_start| luma[col_start..].iter().step_by(rowstride))
        .flat_map(gradient_hash_impl)
}

fn double_gradient_hash<'a, T: PartialOrd>(
    luma: &'a [T],
    rowstride: usize,
) -> impl Iterator<Item = bool> + 'a {
    gradient_hash(luma, rowstride).chain(vert_gradient_hash(luma, rowstride))
}
