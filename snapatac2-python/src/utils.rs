mod anndata;

pub use self::anndata::{PyAnnData, AnnDataLike, RustAnnDataLike};

use pyo3::{
    prelude::*,
    types::PyIterator,
    PyResult, Python,
};
use numpy::{Element, PyReadonlyArrayDyn, PyReadonlyArray, Ix1, Ix2, PyArray, IntoPyArray};
use snapatac2_core::preprocessing::{Transcript, read_transcripts_from_gff, read_transcripts_from_gtf};
use snapatac2_core::utils::similarity;

use bed_utils::{bed, bed::GenomicRange, bed::BED};
use std::io::BufReader;
use std::{str::FromStr, fs::File};
use std::path::{Path, PathBuf};
use flate2::read::MultiGzDecoder;
use linreg::lin_reg_imprecise;
use linfa::{DatasetBase, traits::{Fit, Predict}};
use linfa_clustering::KMeans;
use rand_core::SeedableRng;
use rand_isaac::Isaac64Rng;
use nalgebra_sparse::CsrMatrix;

macro_rules! with_sparsity_pattern {
    ($dtype:expr, $indices:expr, $indptr:expr, $n:expr, $fun:ident) => {
        match $dtype {
            "int32" => {
                let indices_ = $indices.extract::<PyReadonlyArray<i32, Ix1>>()?;
                let indptr_ = $indptr.extract::<PyReadonlyArray<i32, Ix1>>()?;
                $fun!(to_sparsity_pattern(&indptr_, &indices_, $n)?)
            },
            "int64" => {
                let indices_ = $indices.extract::<PyReadonlyArray<i64, Ix1>>()?;
                let indptr_ = $indptr.extract::<PyReadonlyArray<i64, Ix1>>()?;
                $fun!(to_sparsity_pattern(&indptr_, &indices_, $n)?)
            },
            ty => panic!("{}", ty),
        }
    }
}
 

#[pyfunction]
pub(crate) fn jaccard_similarity<'py>(
    py: Python<'py>,
    mat: &'py PyAny,
    other: Option<&'py PyAny>,
    weights: Option<PyReadonlyArray<f64, Ix1>>,
) -> PyResult<&'py PyArray<f64, Ix2>> {
    let weights_ = match weights {
        None => None,
        Some(ref ws) => Some(ws.as_slice().unwrap()),
    };

    macro_rules! with_csr {
        ($mat:expr) => {
            match other {
                None => Ok(similarity::jaccard($mat, weights_).into_pyarray(py)),
                Some(mat2) => {
                    macro_rules! xxx {
                        ($m:expr) => { Ok(similarity::jaccard2($mat, $m, weights_).into_pyarray(py)) };
                    }
                    let shape: Vec<usize> = mat2.getattr("shape")?.extract()?;
                    with_sparsity_pattern!(
                        mat2.getattr("indices")?.getattr("dtype")?.getattr("name")?.extract()?,
                        mat2.getattr("indices")?,
                        mat2.getattr("indptr")?,
                        shape[1],
                        xxx
                    )
                },
            }
        };
    }

    let shape: Vec<usize> = mat.getattr("shape")?.extract()?;
    with_sparsity_pattern!(
        mat.getattr("indices")?.getattr("dtype")?.getattr("name")?.extract()?,
        mat.getattr("indices")?,
        mat.getattr("indptr")?,
        shape[1],
        with_csr
    )
}

fn to_sparsity_pattern<'py, I>(
    indptr_: &'py PyReadonlyArray<I, Ix1>,
    indices_: &'py PyReadonlyArray<I, Ix1>,
    n: usize
) -> PyResult<similarity::BorrowedSparsityPattern<'py, I>>
where
    I: Element,
{
    let indptr = indptr_.as_slice().unwrap();
    let indices = indices_.as_slice().unwrap();
    Ok(similarity::BorrowedSparsityPattern::new(indptr, indices, n))
}

#[pyfunction]
pub(crate) fn cosine_similarity<'py>(
    py: Python<'py>,
    mat: &'py PyAny,
    other: Option<&'py PyAny>,
    weights: Option<PyReadonlyArray<f64, Ix1>>,
) -> PyResult<&'py PyArray<f64, Ix2>> {
    let weights_ = match weights {
        None => None,
        Some(ref ws) => Some(ws.as_slice().unwrap()),
    };
    match other {
        None => Ok(similarity::cosine(csr_to_rust(mat)?, weights_).into_pyarray(py)),
        Some(mat2) => Ok(
            similarity::cosine2(
                csr_to_rust(mat)?,
                csr_to_rust(mat2)?,
                weights_,
            ).into_pyarray(py)
        ),
    }
}

#[pyfunction]
pub(crate) fn pearson<'py>(
    py: Python<'py>,
    mat: &'py PyAny,
    other: &'py PyAny,
) -> PyResult<PyObject> {
    match mat.getattr("dtype")?.getattr("name")?.extract()? {
        "float32" => {
            let mat_ = mat.extract::<PyReadonlyArray<f32, Ix2>>()?.to_owned_array();
            let other_ = other.extract::<PyReadonlyArray<f32, Ix2>>()?.to_owned_array();
            Ok(similarity::pearson2(mat_, other_).into_pyarray(py).to_object(py))
        },
        "float64" => {
            let mat_ = mat.extract::<PyReadonlyArray<f64, Ix2>>()?.to_owned_array();
            let other_ = other.extract::<PyReadonlyArray<f64, Ix2>>()?.to_owned_array();
            Ok(similarity::pearson2(mat_, other_).into_pyarray(py).to_object(py))
        },
        ty => panic!("Cannot compute correlation for type {}", ty),
    }
}

#[pyfunction]
pub(crate) fn spearman<'py>(
    py: Python<'py>,
    mat: &'py PyAny,
    other: &'py PyAny,
) -> PyResult<PyObject> {
    match mat.getattr("dtype")?.getattr("name")?.extract()? {
        "float32" => {
            let mat_ = mat.extract::<PyReadonlyArray<f32, Ix2>>()?.to_owned_array();
            match other.getattr("dtype")?.getattr("name")?.extract()? {
                "float32" => {
                    let other_ = other.extract::<PyReadonlyArray<f32, Ix2>>()?.to_owned_array();
                    Ok(similarity::spearman2(mat_, other_).into_pyarray(py).to_object(py))
                },
                "float64" => {
                    let other_ = other.extract::<PyReadonlyArray<f64, Ix2>>()?.to_owned_array();
                    Ok(similarity::spearman2(mat_, other_).into_pyarray(py).to_object(py))
                },
                ty => panic!("Cannot compute correlation for type {}", ty),
            }
        },
        "float64" => {
            let mat_ = mat.extract::<PyReadonlyArray<f64, Ix2>>()?.to_owned_array();
            match other.getattr("dtype")?.getattr("name")?.extract()? {
                "float32" => {
                    let other_ = other.extract::<PyReadonlyArray<f32, Ix2>>()?.to_owned_array();
                    Ok(similarity::spearman2(mat_, other_).into_pyarray(py).to_object(py))
                },
                "float64" => {
                    let other_ = other.extract::<PyReadonlyArray<f64, Ix2>>()?.to_owned_array();
                    Ok(similarity::spearman2(mat_, other_).into_pyarray(py).to_object(py))
                },
                ty => panic!("Cannot compute correlation for type {}", ty),
            }
        },
        ty => panic!("Cannot compute correlation for type {}", ty),
    }
}

fn csr_to_rust<'py>(csr: &'py PyAny) -> PyResult<CsrMatrix<f64>> {
    let shape: Vec<usize> = csr.getattr("shape")?.extract()?;
    let indices = cast_pyarray(csr.getattr("indices")?)?;
    let indptr = cast_pyarray(csr.getattr("indptr")?)?;
    let data = cast_pyarray(csr.getattr("data")?)?;
    Ok(CsrMatrix::try_from_csr_data(
        shape[0], shape[1], indptr, indices, data,
    ).unwrap())
}

fn cast_pyarray<'py, T: Element>(arr: &'py PyAny) -> PyResult<Vec<T>> {
    let vec = match arr.getattr("dtype")?.getattr("name")?.extract()? {
        "uint32" => arr.extract::<PyReadonlyArrayDyn<u32>>()?.cast(false)?.to_vec().unwrap(),
        "int32" => arr.extract::<PyReadonlyArrayDyn<i32>>()?.cast(false)?.to_vec().unwrap(),
        "uint64" => arr.extract::<PyReadonlyArrayDyn<u64>>()?.cast(false)?.to_vec().unwrap(),
        "int64" => arr.extract::<PyReadonlyArrayDyn<i64>>()?.cast(false)?.to_vec().unwrap(),
        "float32" => arr.extract::<PyReadonlyArrayDyn<f32>>()?.cast(false)?.to_vec().unwrap(),
        "float64" => arr.extract::<PyReadonlyArrayDyn<f64>>()?.cast(false)?.to_vec().unwrap(),
        ty => panic!("cannot cast type {}", ty),
    };
    Ok(vec)
}

/// Simple linear regression
#[pyfunction]
pub(crate) fn simple_lin_reg(py_iter: &PyIterator) -> PyResult<(f64, f64)> {
    Ok(lin_reg_imprecise(py_iter.map(|x| x.unwrap().extract().unwrap())).unwrap())
}

/// Perform regression
#[pyfunction]
pub(crate) fn jm_regress(
    jm_: PyReadonlyArrayDyn<'_, f64>,
    count_: PyReadonlyArrayDyn<'_, f64>,
) -> PyResult<(f64, f64)> {
    let jm = &jm_.as_array();
    let n_row = jm.shape()[0];
    let count = &count_.as_array();
    let iter = (0..n_row).flat_map(|i| (i+1..n_row)
        .map(move |j| (1.0 / (1.0 / count[[i, 0]] + 1.0 / count[[j, 0]] - 1.0), jm[[i, j]]))
    );
    Ok(lin_reg_imprecise(iter).unwrap())
}

/// Read genomic regions from a bed file.
/// Returns a list of strings
#[pyfunction]
pub(crate) fn read_regions(file: PathBuf) -> Vec<String> {
    let mut reader = bed::io::Reader::new(open_file(file), None);
    reader.records::<GenomicRange>().map(|x| x.unwrap().pretty_show()).collect()
}

#[pyfunction]
pub(crate) fn intersect_bed<'py>(py: Python<'py>, regions: &'py PyAny, bed_file: &str) -> PyResult<Vec<bool>> {
    let bed_tree: bed::tree::BedTree<()> = bed::io::Reader::new(open_file(bed_file), None)
        .into_records().map(|x: Result<BED<3>, _>| (x.unwrap(), ())).collect();
    let res = PyIterator::from_object(py, regions)?
        .map(|x| bed_tree.is_overlapped(&GenomicRange::from_str(x.unwrap().extract().unwrap()).unwrap()))
        .collect();
    Ok(res)
}

#[pyfunction]
pub(crate) fn kmeans<'py>(
    py: Python<'py>,
    n_clusters: usize,
    observations_: PyReadonlyArray<'_, f64, Ix2>,
) -> PyResult<&'py PyArray<usize, Ix1>> {
    let seed = 42;
    let rng: Isaac64Rng = SeedableRng::seed_from_u64(seed);
    let observations = DatasetBase::from(observations_.as_array());
    let model = KMeans::params_with_rng(n_clusters, rng)
        .fit(&observations)
        .expect("KMeans fitted");
    Ok(model.predict(observations).targets.into_pyarray(py))
}

/// Open a file, possibly compressed. Supports gzip and zstd.
pub(crate) fn open_file<P: AsRef<Path>>(file: P) -> Box<dyn std::io::Read> {
    match detect_compression(file.as_ref()) {
        Compression::Gzip => Box::new(MultiGzDecoder::new(File::open(file.as_ref()).unwrap())),
        Compression::Zstd => {
            let r = zstd::stream::read::Decoder::new(File::open(file.as_ref()).unwrap()).unwrap();
            Box::new(r)
        },
        Compression::None => Box::new(File::open(file.as_ref()).unwrap()),
    }
}

enum Compression {
    Gzip,
    Zstd,
    None,
}

/// Determine the file compression type. Supports gzip and zstd.
fn detect_compression<P: AsRef<Path>>(file: P) -> Compression {
    if MultiGzDecoder::new(File::open(file.as_ref()).unwrap()).header().is_some() {
        Compression::Gzip
    } else if let Some(ext) = file.as_ref().extension() {
        if ext == "zst" {
            Compression::Zstd
        } else {
            Compression::None
        }
    } else {
        Compression::None
    }
}

pub fn read_transcripts<P: AsRef<std::path::Path>>(file_path: P) -> Vec<Transcript> {
    let path = if file_path.as_ref().extension().unwrap() == "gz" {
        file_path.as_ref().file_stem().unwrap().as_ref()
    } else {
        file_path.as_ref()
    };
    if path.extension().unwrap() == "gff" {
        read_transcripts_from_gff(BufReader::new(open_file(file_path))).unwrap()
    } else if path.extension().unwrap() == "gtf" {
        read_transcripts_from_gtf(BufReader::new(open_file(file_path))).unwrap()
    } else {
        read_transcripts_from_gff(BufReader::new(open_file(file_path.as_ref())))
            .unwrap_or_else(|_| read_transcripts_from_gtf(BufReader::new(open_file(file_path))).unwrap())
    }
}