#![deny(warnings)]
use pretty_env_logger;
use std::env;
use warp::{http::StatusCode, Filter, Rejection, Reply};
use bytes::BufMut;

type Result<T> = std::result::Result<T, Rejection>;
use mongodb::{gridfs::GridFsBucket, Client};
use hyper::{Body,Response};
use futures::AsyncReadExt;

async fn download_handler(filename: String, bucket: GridFsBucket) -> Result<impl Reply> {
    //channel to pipe output from async reader
    let (sender, body) = Body::channel();

    let mut download_stream = bucket
        .open_download_stream_by_name(filename, None)
        .await
        .expect("should be able to download data to bucket");


    tokio::spawn(async move {
        let mut sender = sender;

        loop {
            let mut buf = [0; 1024000]; 
            let byte_count = download_stream.read(&mut buf).await.expect("should get bytes");
            if byte_count == 0 {
                break;
            }

            //copy data out of mut ref. 
            //TODO: see if there is a better way.
            let mut buf2 = vec![];
            buf2.put(&buf[..byte_count]);

            sender.send_data(buf2.into()).await.expect("should be able to send");
        }
    });

    let resp = Response::new(body);
    
    Ok(resp)
}

pub async fn health_handler() -> Result<impl Reply> {
    Ok(StatusCode::OK)
}

#[tokio::main]    
async fn main() {
    pretty_env_logger::init();

    let mongo_endpoint = env::var("ENDPOINT").expect("$ENDPOINT is not set");
    let mongo_table = env::var("TABLE").expect("$TABLE is not set");

    let client = Client::with_uri_str(&mongo_endpoint).await.expect("should be able to setup MondoDB client");
    let db = client.database(&mongo_table);
    let bucket = db.gridfs_bucket(None);

    let bucket = warp::any().map(move || bucket.clone());

    let health_route = warp::path!("health").and_then(health_handler);

    let media = warp::path("violin");
    let media_routes = media
        .and(warp::get())
        .and(warp::path::param())
        .and(bucket)
        .and_then(download_handler)
        .or(
            media.and(warp::options().map(warp::reply))
        );
        

    let routes = warp::path("api").and(
        warp::path("media").and(
            warp::path("v1").and(
                health_route
                .or(media_routes)
            )
        )
    );

    warp::serve(routes).run(([0, 0, 0, 0], 80)).await;

}