//#![deny(warnings)]
use pretty_env_logger;
use std::env;
use warp::{http::{StatusCode}, Filter, Rejection, Reply, reply::Response, reject};
use mongodb::{bson::doc, gridfs::GridFsBucket, gridfs::GridFsDownloadStream, gridfs::FilesCollectionDocument, Client};
use hyper::Body;
use futures::{future::Either,future, ready, stream, TryStreamExt, Stream, FutureExt, StreamExt};//, io::AsyncSeek};

use tokio_util::io::poll_read_buf;
use tokio_util::compat::Compat;
//use tokio::io::AsyncSeekExt;
use tokio::io::AsyncReadExt;

use tokio_util::compat::FuturesAsyncReadCompatExt;

use headers::{
    AcceptRanges, ContentLength, ContentType, ContentRange, HeaderMap, HeaderMapExt, IfModifiedSince, IfRange,
    IfUnmodifiedSince, LastModified, Range,
};

use mime_guess;

use bytes::{Bytes, BytesMut};
// use futures_util::future::Either;
// use futures_util::{}
use std::io;
use std::pin::Pin;
use std::task::Poll;

// impl AsyncSeek for GridFsDownloadStream {
//     // fn poll_read(
//     //     self: Pin<&mut Self>,
//     //     cx: &mut Context<'_>,
//     //     buf: &mut [u8],
//     // ) -> Poll<std::result::Result<usize, futures_util::io::Error>> {
//     fn poll_seek(
//         self: Pin<&mut Self>,
//         cx: &mut Context<'_>,
//         pos: SeekFrom
//     ) -> Poll<Result<u64, Error>> {
//         let stream = self.get_mut();

//         let result = match &mut stream.state {
//             State::Idle(idle) => {
//                 let Idle { buffer, cursor } = idle.take().unwrap();

//                 if !buffer.is_empty() {
//                     Ok((buffer, cursor))
//                 } else {
//                     let chunks_in_buf = FilesCollectionDocument::n_from_vals(
//                         buf.len() as u64,
//                         stream.file.chunk_size_bytes,
//                     );
//                     // We should read from current_n to chunks_in_buf + current_n, or, if that would
//                     // exceed the total number of chunks in the file, to the last chunk in the file.
//                     let final_n = std::cmp::min(chunks_in_buf + stream.current_n, stream.file.n());
//                     let n_range = stream.current_n..final_n;

//                     stream.current_n = final_n;

//                     let new_future = stream.state.set_busy(
//                         get_bytes(
//                             cursor,
//                             buffer,
//                             n_range,
//                             stream.file.chunk_size_bytes,
//                             stream.file.length,
//                         )
//                         .boxed(),
//                     );

//                     match new_future.poll_unpin(cx) {
//                         Poll::Ready(result) => result,
//                         Poll::Pending => return Poll::Pending,
//                     }
//                 }
//             }

//             State::Busy(future) => match future.poll_unpin(cx) {
//                 Poll::Ready(result) => result,
//                 Poll::Pending => return Poll::Pending,
//             },
//             State::Done => return Poll::Ready(Ok(0)),
//         };

//         match result {
//             Ok((mut buffer, cursor)) => {
//                 let bytes_to_write = std::cmp::min(buffer.len(), buf.len());
//                 buf[..bytes_to_write].copy_from_slice(buffer.drain(0..bytes_to_write).as_slice());

//                 stream.state = if !buffer.is_empty() || cursor.has_next() {
//                     State::Idle(Some(Idle { buffer, cursor }))
//                 } else {
//                     State::Done
//                 };

//                 Poll::Ready(Ok(bytes_to_write))
//             }
//             Err(error) => {
//                 stream.state = State::Done;
//                 Poll::Ready(Err(error.into_futures_io_error()))
//             }
//         }
//     }
// }

// async fn get_bytes(
//     mut cursor: Box<Cursor<Chunk<'static>>>,
//     mut buffer: Vec<u8>,
//     n_range: Range<u32>,
//     chunk_size_bytes: u32,
//     file_len: u64,
// ) -> Result<(Vec<u8>, Box<Cursor<Chunk<'static>>>)> {
//     for n in n_range {
//         if !cursor.advance().await? {
//             return Err(ErrorKind::GridFs(GridFsErrorKind::MissingChunk { n }).into());
//         }

//         let chunk = cursor.deserialize_current()?;
//         let chunk_bytes = chunk.data.bytes;

//         if chunk.n != n {
//             return Err(ErrorKind::GridFs(GridFsErrorKind::MissingChunk { n }).into());
//         }

//         let expected_len =
//             FilesCollectionDocument::expected_chunk_length_from_vals(file_len, chunk_size_bytes, n);
//         if chunk_bytes.len() != (expected_len as usize) {
//             return Err(ErrorKind::GridFs(GridFsErrorKind::WrongSizeChunk {
//                 actual_size: chunk_bytes.len(),
//                 expected_size: expected_len,
//                 n,
//             })
//             .into());
//         }

//         buffer.extend_from_slice(chunk_bytes);
//     }

//     Ok((buffer, cursor))
// }


#[derive(Debug)]
struct Conditionals {
    if_modified_since: Option<IfModifiedSince>,
    if_unmodified_since: Option<IfUnmodifiedSince>,
    if_range: Option<IfRange>,
    range: Option<Range>,
}

enum Cond {
    NoBody(Response),
    WithBody(Option<Range>),
}

impl Conditionals {
    fn check(self, last_modified: Option<LastModified>) -> Cond {
        if let Some(since) = self.if_unmodified_since {
            let precondition = last_modified
                .map(|time| since.precondition_passes(time.into()))
                .unwrap_or(false);

            if !precondition {
                let mut res = Response::new(Body::empty());
                *res.status_mut() = StatusCode::PRECONDITION_FAILED;
                return Cond::NoBody(res);
            }
        }

        if let Some(since) = self.if_modified_since {
            let unmodified = last_modified
                .map(|time| !since.is_modified(time.into()))
                // no last_modified means its always modified
                .unwrap_or(false);
            if unmodified {
                let mut res = Response::new(Body::empty());
                *res.status_mut() = StatusCode::NOT_MODIFIED;
                return Cond::NoBody(res);
            }
        }

        if let Some(if_range) = self.if_range {
            let can_range = !if_range.is_modified(None, last_modified.as_ref());

            if !can_range {
                return Cond::WithBody(None);
            }
        }

        Cond::WithBody(self.range)
    }
}

struct BadRange;

fn bytes_range(range: Option<Range>, max_len: u64) -> Result<(u64, u64), BadRange> {
    use std::ops::Bound;

    let range = if let Some(range) = range {
        range
    } else {
        return Ok((0, max_len));
    };

    let ret = range
        .iter()
        .map(|(start, end)| {
            let start = match start {
                Bound::Unbounded => 0,
                Bound::Included(s) => s,
                Bound::Excluded(s) => s + 1,
            };

            let end = match end {
                Bound::Unbounded => max_len,
                Bound::Included(s) => {
                    // For the special case where s == the file size
                    if s == max_len {
                        s
                    } else {
                        s + 1
                    }
                }
                Bound::Excluded(s) => s,
            };

            if start < end && end <= max_len {
                Ok((start, end))
            } else {
                Err(BadRange)
            }
        })
        .next()
        .unwrap_or(Ok((0, max_len)));
    ret
}

fn file_stream(
    mut file: Compat<GridFsDownloadStream>,
    buf_size: usize,
    (start, end): (u64, u64),
) -> impl Stream<Item = Result<Bytes, io::Error>> + Send {
   // use std::io::SeekFrom;

    let seek = async move {
        if start != 0 {
            // const start: usize = start;
            let mut buffer = Vec::with_capacity(start as usize);
            file.read_exact(&mut buffer).await?;
            //file.seek(SeekFrom::Start(start)).await?;
        }
        Ok(file)
    };

    seek.into_stream()
        .map(move |result| {
            let mut buf = BytesMut::new();
            let mut len = end - start;
            let mut f = match result {
                Ok(f) => f,
                Err(f) => return Either::Left(stream::once(future::err(f))),
            };

            Either::Right(stream::poll_fn(move |cx| {
                if len == 0 {
                    return Poll::Ready(None);
                }
                reserve_at_least(&mut buf, buf_size);

                let n = match ready!(poll_read_buf(Pin::new(&mut f), cx, &mut buf)) {
                    Ok(n) => n as u64,
                    Err(err) => {
                        return Poll::Ready(Some(Err(err)));
                    }
                };

                if n == 0 {
                    return Poll::Ready(None);
                }

                let mut chunk = buf.split().freeze();
                if n > len {
                    chunk = chunk.split_to(len as usize);
                    len = 0;
                } else {
                    len -= n;
                }

                Poll::Ready(Some(Ok(chunk)))
            }))
        })
        .flatten()
}

fn reserve_at_least(buf: &mut BytesMut, cap: usize) {
    if buf.capacity() - buf.len() < cap {
        buf.reserve(cap);
    }
}

async fn download_handler(filename: String, bucket: GridFsBucket, conditionals: Conditionals) -> Result<impl Reply, Rejection> {
    let filter = doc!{"filename": filename.clone()};
    match bucket.find(filter, None).await {
        Ok(cursor) => {
            let file_collections: Result<Vec<FilesCollectionDocument>,_> = cursor.try_collect().await;
            match file_collections {
                Ok(metas) => {
                    let _chunk_size = metas[0].chunk_size_bytes;
                    let mut length = metas[0].length;
                    let modified = Some(LastModified::from(metas[0].upload_date.to_system_time()));

                    let resp = match conditionals.check(modified) {
                        Cond::NoBody(resp) => resp,
                        Cond::WithBody(range) => {
                            match bytes_range(range, length) {
                                Ok((start, end)) => {
                                    let sub_len = end - start;
                                    let buf_size = 8_192;

                                    let download_stream = bucket
                                        .open_download_stream_by_name(filename.clone(), None)
                                        .await
                                        .expect("should be able to download data to bucket");

                                    let stream = file_stream(download_stream.compat(), buf_size, (start, end));

                                    let body = Body::wrap_stream(stream);

                                    let mut resp = Response::new(body);

                                    if sub_len != length {
                                        *resp.status_mut() = StatusCode::PARTIAL_CONTENT;
                                        resp.headers_mut().typed_insert(
                                            ContentRange::bytes(start..end, length).expect("valid ContentRange"),
                                        );

                                        length = sub_len;
                                    }

                                    let mime = mime_guess::from_path(filename.clone()).first_or_octet_stream();
                                    resp.headers_mut().typed_insert(ContentType::from(mime));

                                    resp.headers_mut().typed_insert(ContentLength(length));
                                    resp.headers_mut().typed_insert(AcceptRanges::bytes());

                                    if let Some(last_modified) = modified {
                                        resp.headers_mut().typed_insert(last_modified);
                                    }

                                    resp
                                },
                                Err(_) => {
                                    // bad byte range
                                    let mut resp = Response::new(Body::empty());
                                    *resp.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
                                    resp.headers_mut()
                                        .typed_insert(ContentRange::unsatisfied_bytes(length));
                                    resp
                                }
                            }
                        }
                    };
                    
                    Ok(resp)
                },
                Err(_) => return Err(reject::not_found())
            }
        },
        Err(_) => {
            println!("no file");
            return Err(reject::not_found())
        }
    }
}

pub async fn health_handler() -> Result<impl Reply, Rejection> {
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
        .and(
            warp::header::headers_cloned()
            .map(|h: HeaderMap| {
                let if_modified_since = h.typed_get::<IfModifiedSince>();
                let if_unmodified_since = h.typed_get::<IfUnmodifiedSince>();
                let if_range = h.typed_get::<IfRange>();
                let range = h.typed_get::<Range>();
                Conditionals {
                    if_modified_since,
                    if_unmodified_since,
                    if_range,
                    range,
                }
            })
        )
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