//! Opt-in local evidence runner. Inputs are JSON [{modId,vpks}], never copied to source.
fn main(){
 let game=std::env::var("DMM_HEALTH_GAME").expect("DMM_HEALTH_GAME");
 let input=std::env::var("DMM_HEALTH_INPUT").expect("DMM_HEALTH_INPUT");
 let data:serde_json::Value=serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
 let inputs=data.as_array().unwrap().iter().map(|r|(r["modId"].as_str().unwrap().to_owned(),r["vpks"].as_array().unwrap().iter().map(|p|std::path::PathBuf::from(p.as_str().unwrap())).collect())).collect();
 let report=health_engine::check(std::path::Path::new(&game),inputs,std::env::var("DMM_HEALTH_BUILD").ok().and_then(|s|s.parse().ok()),|n,t|eprintln!("checked {n}/{t}")).unwrap();
 println!("{}",serde_json::to_string_pretty(&report).unwrap());
}
