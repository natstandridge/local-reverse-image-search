use crate::config::Config;

// use std::path::Path;
// use cv::{feature::akaze::Akaze, KeyPoint, BitArray};
// use cv::feature::akaze
use akaze::{Akaze, KeyPoint};
use serde::ser::SerializeStruct;
use serde::{Serialize, Deserialize};
use std::iter::zip;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use bitarray::BitArray;
// use image::dynimage::DynamicImage;
// use std::path::Path;
use kdam::{tqdm, BarExt};
use image::imageops::FilterType;
// use std::collections::HashMap;
use std::fmt;
use console::style;
use indicatif::{HumanDuration, MultiProgress, ProgressBar, ProgressStyle};
use num_cpus;
use sled::{Db, IVec};
use bincode;
use std::convert::From;
use serde::{Serializer, Deserializer};
use kdtree::KdTree;
use kdtree::ErrorKind;
use kdtree::distance::squared_euclidean;

#[derive(Debug, Serialize, Deserialize)]
pub struct ImgInfo {
    pub path: String,
    pub num_matches: u32
}

impl fmt::Display for ImgInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "IMGINFO --> path: {}", self.path)
    }
}


pub fn extract_single(cache: Arc<Mutex<Db>>, resize_dims: [u32; 2], path: &String) -> Option<(Vec<KeyPoint>, Vec<BitArray<64>>)> {

    let cache_mguard = cache.lock().unwrap();
    let res = cache_mguard.get(path);
    drop(cache_mguard);
 
    match res {

        Ok(res) => match res {

            Some(val) => {
                panic!("a value was returned from the database somehow...");
            },

            None => {
                /* make new feature extractor */
                let akaze = Akaze::default();
            
                /* extract keypoints and descriptors */
                let [nwidth, nheight] = resize_dims;
                let filter = FilterType::Nearest;
                let img = match image::open(&path) {

                    Ok(img) => img.resize(nwidth, nheight, filter),

                    Err(err) => {
                        // println!("\n------------------");
                        // println!("{}: unable to open {}\n\n{}", style("ERROR").bold().bright().red(), style(path).bold().bright(), err);
                        // println!("------------------\n");
                        return None
                    }
                };
            
                /* return extracted info */
                let (keypoints, descriptors) = akaze.extract(&img);

                /* add to database */
                // let data = 
                // let _ = cache.insert(path, (keypoints, descriptors)).unwrap().unwrap();

                /* return */
                Some((keypoints, descriptors))
            }
        },
        Err(err) => panic!("error with database")
    }
}

pub fn bitarray_to_floatvec(ba: &BitArray<64>) -> Vec<f32> {
    
    // let mut output: [f32; 64];
    // for (i, element) in ba.iter().enumerate() {
    //     output[i] = element.clone() as f32;
    // }

    let mut output = Vec::new();
    for byte in ba.iter() {
        output.push(byte.clone() as f32);
    }

    output
}

pub fn floatvec_to_floatarray(fv: &Vec<f32>) -> [f32; 64] {

    let mut desc_array: [f32; 64] = [0 as f32; 64];

    for (bytenum, byte) in fv.iter().enumerate() {
        desc_array[bytenum] = byte.clone();
    }

    desc_array
}


fn get_num_matches(ratio_test_ratio: f32, descs_query: &Vec<BitArray<64>>, descs_search: (&Vec<BitArray<64>>, &String)) -> u32 {

    let (descs, path) = descs_search;
    
    /* fit nearest neighbors classifier to query descriptors */
    let mut kdtree = KdTree::new(64);

    for (descnum, desc_ba) in descs.iter().enumerate() {

        let desc_vec: Vec<f32> = bitarray_to_floatvec(desc_ba);
        let desc_array: [f32; 64] = floatvec_to_floatarray(&desc_vec);
        let _ = kdtree.add(desc_array, descnum);
    };
    // let mut tree = RTree::bulk_load(rtreeelements);

    // let mut tokens = Vec::new();
    // let mut elements = granne::angular::Vectors::new();

    /* add query descriptor points to kdtree */
    // for (i, desc) in descs.iter().enumerate() {

    //     /* convert from u8 to f32 for search compatibility */
    //     let vecf32 = bitarray_to_floatarray(desc);

    //     /* add to collection */
    //     tokens.push(i);
    //     elements.push(&granne::angular::Vector::from(Vec::from(vecf32)));
    //     // tree.insert([i, vecf32]);
    // }

    // building the index
    // let build_config = granne::BuildConfig::default().show_progress(false).max_search(10); // increase this for better results

    // let mut builder = granne::GranneBuilder::new(build_config, elements);

    // builder.build();

    // let index = builder.get_index();

    let mut num_matches: u32 = 0;

    for qdesc in descs_query.iter() {

        /* convert query descriptor to float array */
        let qvec = bitarray_to_floatvec(qdesc);
        let qarray = floatvec_to_floatarray(&qvec);

        // let res = index.search(&granne::angular::Vector::from(Vec::from(qvecf32)), 200, 10);
        let res = kdtree.nearest(&qarray, 10, &squared_euclidean).unwrap();

        /* do ratio test */
        if res.len() > 1 && res[0].0 < ratio_test_ratio * res[1].0  {
            num_matches += 1;
        }

        // pb.inc(1);
    }

    // pb.finish_with_message(format!(" --> {} matches", num_matches));

    num_matches
}

pub fn calculate_similarities(cache: Arc<Mutex<Db>>, cfg: &Config, query_desc: &Vec<BitArray<64>>, search_paths: Vec<String>) -> (Arc<Mutex<Vec<ImgInfo>>>, Vec<String>) {
    
    let info: Arc<Mutex<Vec<ImgInfo>>> = Arc::new(Mutex::new(Vec::new()));

    let failed_paths_arc: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    
    let mut handles = Vec::new();
    let pb = Arc::new(Mutex::new(tqdm!(total=search_paths.len(), desc="extracting features")));

    // let m = MultiProgress::new();
    // let pb = ProgressBar::new(search_paths.len() as u64);

    /* determine num workers */
    let num_workers = match cfg.num_workers {
        0 => num_cpus::get(),
        _ => cfg.num_workers as usize
    };
    println!("{} workers", num_workers);

    // let pool = ThreadPool::new(num_workers);
    // let pool = rayon::ThreadPoolBuilder::new().num_threads(num_workers).build().unwrap();

    let sp = search_paths.to_owned();
    let chunks = sp.chunks(sp.len() / num_workers);
    let mut chunks_owned = Vec::new();

    for chunk in chunks {
        chunks_owned.push(chunk.to_owned());
    }

    let ratio_test_ratio = cfg.ratio_test_ratio;

    /* multithreaded batch feature extraction */
    for chunk in chunks_owned {
        // println!("\n\nchunk len: {}", chunk.len());
        let thisinfo = info.clone();
        let thispb = pb.clone();
        let this_qdesc = query_desc.clone();
        let resize_dims = cfg.resize_dimensions;
        let thiscache = cache.clone();
        let thisfailedpaths = failed_paths_arc.clone();

        // let pb = m.add(ProgressBar::new(0));

        // pool.execute(move || {
        handles.push(thread::spawn(move || {

            // set_current_thread_priority(ThreadPriority::Max).unwrap();

            for path in chunk.to_owned() {   
                
                let mut msg: String = String::new();
                
                /* get keypoints and descriptors for this search image */
                match extract_single(thiscache.clone(), resize_dims, &path) {

                    Some((keypoints, descriptors)) => {

                        /* calculte similarity to query image (num matches) */
                        let num_matches = get_num_matches(ratio_test_ratio, &this_qdesc, (&descriptors, &path));
            
                        msg = format!("{:>6} matches <- {}", num_matches, style(path.clone()).bold().blue());
            
                        /* add extracted info to output */
                        let mut thisinfo_guard = thisinfo.lock().unwrap();
                        thisinfo_guard.push(ImgInfo { path, num_matches });
                        drop(thisinfo_guard);
                    },

                    None => {
                        msg = format!("{}: unable to open {}, skipping", style("ERROR").bold().bright().red(), style(path.clone()).bold());
                        let mut failed_paths = thisfailedpaths.lock().unwrap();
                        failed_paths.push(path.clone());
                    }
                }

                let mut p = thispb.lock().unwrap();
                p.update(1);
                p.write(msg);
                drop(p);
            }
        }));
    }
    eprint!("\n");

    /* make sure all threads are finished before returning */
    for handle in handles {
        handle.join().unwrap();
    }
    // m.clear().unwrap();

    let failed_paths = failed_paths_arc.lock().unwrap().iter().map(|x| x.clone()).collect();

    (info, failed_paths)
}