#[macro_use] extern crate log;

use crate::http_client::{GetRequestTest, http_get, http_get_with_header_chunked, ChunkPattern, ConnAddr, GetRequest, HttpGetResult, HEADER_SEPARATOR_STR};
use std::time::Duration;
use crate::http_client::ClientHeader::{AutoGenerated, Custom};
use std::ops::Range;
use std::sync::mpsc;
use std::sync::mpsc::Receiver;
use crossbeam_utils::thread;
use colored::*;


mod http_client;

const DEFAULT_PORT: u16 = 7878;

const LARGE_FILE_SIZE: usize = 8 * 1024 * 1024 * 1024;
const LARGE_FILE_REQUEST_PATH: &str = "/large";
const FILE_PARTIALLY_CACHED_REQUEST_PATH: &str = "/partially-cached";
const FILE_PARTIALLY_CACHED_SIZE: usize = 100;
const FILE_ZERO_SMALL_SIZE: usize = 102400;

struct PathGenerator {
    range: Range<i32>,
}
impl PathGenerator {
    fn generate(&mut self) -> String {
        format!("/test_{}", self.range.next().unwrap())
    }
}

struct FlexoTest {
    description: &'static str,
    action: fn(&mut PathGenerator, &'static str) -> ()
}

#[derive(Debug, Hash, Clone, PartialEq, Eq)]
enum TestOutcome {
    Success,
    Failure,
}

fn main() {
    env_logger::builder().format_timestamp_millis().init();
    let flexo_test_run_only = match std::env::var("FLEXO_TEST_RUN_ONLY") {
        Ok(n) if n.is_empty() => None,
        Err(_) => None,
        Ok(n) => Some(n),
    };

    let all_tests: Vec<FlexoTest> = vec![
        FlexoTest {
            description: "flexo_test_partial_header",
            action: flexo_test_partial_header,
        },
        FlexoTest {
            description: "flexo_test_malformed_header",
            action: flexo_test_malformed_header,
        },
        FlexoTest {
            description: "flexo_test_persistent_connections_c2s",
            action: flexo_test_persistent_connections_c2s,
        },
        FlexoTest {
            description: "flexo_test_persistent_connections_s2s",
            action: flexo_test_persistent_connections_s2s,
        },
        FlexoTest {
            description: "flexo_test_mirror_selection_slow_mirror",
            action: flexo_test_mirror_selection_slow_mirror,
        },
        FlexoTest {
            description: "flexo_test_download_large_file",
            action: flexo_test_download_large_file,
        },
        FlexoTest {
            description: "flexo_test_download_large_file_cached",
            action: flexo_test_download_large_file_cached,
        },
        FlexoTest {
            description: "flexo_test_parallel_downloads_nonblocking",
            action: flexo_test_parallel_downloads_nonblocking,
        },
        FlexoTest {
            description: "flexo_test_download_large_file_cached_resume",
            action: flexo_test_download_large_file_cached_resume,
        },
        FlexoTest {
            description: "flexo_test_mirror_stalling",
            action: flexo_test_mirror_stalling,
        },
        FlexoTest {
            description: "flexo_test_mirror_stalling_after_header",
            action: flexo_test_mirror_stalling_after_header,
        },
        FlexoTest {
            description: "flexo_test_404_not_found",
            action: flexo_test_404_not_found,
        },
        FlexoTest {
            description: "flexo_test_root_path_400",
            action: flexo_test_root_path_400,
        },
        FlexoTest {
            description: "flexo_test_empty_path_400",
            action: flexo_test_empty_path_400,
        },
        FlexoTest {
            description: "flexo_test_no_content_length",
            action: flexo_test_no_content_length,
        },
        FlexoTest {
            description: "flexo_test_redirect",
            action: flexo_test_redirect,
        },
        FlexoTest {
            description: "flexo_test_file_partially_cached",
            action: flexo_test_file_partially_cached,
        },
        FlexoTest {
            description: "flexo_test_parallel_downloads_same_file",
            action: flexo_test_parallel_downloads_same_file,
        },
    ];
    let tests: Vec<FlexoTest> = all_tests.into_iter().filter(|test| match &flexo_test_run_only {
        Some(f) =>
            test.description == f,
        None =>
            // if the environment variable was not specified, run all tests.
            true,
    }).collect();
    if tests.is_empty() {
        warn!("No tests match the specified criteria.");
        return;
    }
    let max_len = tests.iter().map(|t| t.description.chars().count()).max().unwrap();

    let mut path_generator = PathGenerator {
        range: 0..1000,
    };

    let mut outcomes = vec![];

    for test in tests {
        info!("Starting test {}", test.description);
        let t = thread::scope(|s| {
            s.spawn(|_| {
                (test.action)(&mut path_generator, test.description);
            });
        });
        let outcome = match t {
            Ok(_) => {
                info!("{}: [SUCCESS]", test.description);
                TestOutcome::Success
            }
            Err(_) => {
                info!("{}: [FAILURE]", test.description);
                TestOutcome::Failure
            }
        };
        outcomes.push((test.description, outcome));
    }

    let num_failures = outcomes.iter().filter(|(_, outcome)| outcome == &TestOutcome::Failure).count();

    println!("Test summary:");
    for (testname, outcome) in outcomes {
        let padding = " ".repeat(max_len - testname.chars().count() + 1);
        let suffix = format!("{:?}", outcome).to_uppercase();
        let colored_suffix = match outcome {
            TestOutcome::Success => suffix.green().to_string(),
            TestOutcome::Failure => suffix.red().to_string(),
        };
        println!("{}:{}[{}]", testname, padding, colored_suffix.green());
    }
    let exit_code = match num_failures {
        0 => {
            println!("All test cases have succeeded!");
            0
        },
        1 => {
            println!("A test case has failed!");
            1
        },
        _ => {
            println!("{} test cases have failed!", num_failures);
            1
        }
    };

    std::process::exit(exit_code);
}

fn flexo_test_malformed_header(_path_generator: &mut PathGenerator, testcase: &'static str) {
    let malformed_header = "this is not a valid http header".to_owned();
    let uri1 = GetRequestTest {
        conn_addr: ConnAddr {
            host: "flexo-server".to_owned(),
            port: DEFAULT_PORT,
        },
        get_requests: vec![GetRequest {
            path: "/".to_owned(),
            client_header: Custom(malformed_header),
        }],
        timeout: None,
    };
    let results = http_get(uri1, testcase).unwrap();
    assert_eq!(results.len(), 1);
    let result = results.get(0).unwrap();
    println!("result: {:?}", &result);
    assert_eq!(result.header_result.status_code, 400);
    // Test if the server is still up, i.e., the previous request hasn't crashed it:
    let uri2 = GetRequestTest {
        conn_addr: ConnAddr {
            host: "flexo-server".to_owned(),
            port: DEFAULT_PORT,
        },
        get_requests: vec![GetRequest {
            path: "/status".to_owned(),
            client_header: AutoGenerated,
        }],
        timeout: None,
    };
    let results = http_get(uri2, testcase).unwrap();
    assert_eq!(results.len(), 1);
    let result = results.get(0).unwrap();
    println!("result: {:?}", &result);
    assert_eq!(result.header_result.status_code, 200);
}

fn flexo_test_partial_header(path_generator: &mut PathGenerator, testcase: &'static str) {
    // Sending the header in multiple TCP segments does not cause the server to crash
    let uri = GetRequestTest {
        conn_addr: ConnAddr {
            host: "flexo-server-slow-primary".to_owned(),
            port: DEFAULT_PORT,
        },
        get_requests: vec![GetRequest {
            path: path_generator.generate(),
            client_header: AutoGenerated,
        }],
        timeout: None,
    };
    let pattern = ChunkPattern {
        chunk_size: 3,
        wait_interval: Duration::from_millis(300),
    };
    let results = http_get_with_header_chunked(uri, Some(pattern), testcase).unwrap();
    assert_eq!(results.len(), 1);
    let result = results.get(0).unwrap();
    assert_eq!(result.header_result.status_code, 200);
}


fn flexo_test_persistent_connections_c2s(path_generator: &mut PathGenerator, testcase: &'static str) {
    let request_test = GetRequestTest {
        conn_addr: ConnAddr {
            host: "flexo-server-delay".to_owned(),
            port: DEFAULT_PORT,
        },
        get_requests: vec![
            GetRequest {
                path: path_generator.generate(),
                client_header: AutoGenerated
            },
            GetRequest {
                path: path_generator.generate(),
                client_header: AutoGenerated
            },
            GetRequest {
                path: path_generator.generate(),
                client_header: AutoGenerated
            },
        ],
        // Setting a timeout that slightly exceeds the connection delay from flexo-server-delay: This is used as
        // proof that the connection establishment happens only once, not 3 times. If each request would require
        // a new connection establishment, then a timeout would occur and the test case would fail.
        timeout: Some(Duration::from_millis(220)),
    };
    let results = http_get(request_test, testcase).unwrap();
    assert_eq!(results.len(), 3);
    let all_ok = results.iter().all(|r| r.header_result.status_code == 200);
    assert!(all_ok);
}

fn flexo_test_persistent_connections_s2s(path_generator: &mut PathGenerator, testcase: &'static str) {
    // Connections made from server-to-server (i.e., from flexo to the remote mirror) should be persistent.
    // We can test this only in an indirect manner: Based on the assumption that a short delay happens before
    // the flexo server can connect to the remote mirror, we conclude that if many files have been successfully
    // downloaded within the timeout, only one connection was established between the flexo server and the remote
    // mirror: If a new connection had been used for every request, the timeout would not have been sufficient.
    let get_requests: Vec<GetRequest> = (0..100).map(|_| {
        GetRequest {
            path: path_generator.generate(),
            client_header: AutoGenerated,
        }
    }).collect();
    let request_test = GetRequestTest {
        conn_addr: ConnAddr {
            host: "flexo-server-delay-primary".to_owned(),
            port: DEFAULT_PORT,
        },
        get_requests,
        timeout: Some(Duration::from_secs(5)),
    };
    let results = http_get(request_test, testcase).unwrap();
    assert_eq!(results.len(), 100);
    let all_ok = results.iter().all(|r| r.header_result.status_code == 200);
    assert!(all_ok);
}

fn flexo_test_mirror_selection_slow_mirror(_path_generator: &mut PathGenerator, testcase: &'static str) {
    let get_requests = vec![
        GetRequest {
            path: "/zero".to_owned(),
            client_header: AutoGenerated,
        }
    ];
    let request_test = GetRequestTest {
        conn_addr: ConnAddr {
            host: "flexo-server-slow-primary".to_owned(),
            port: DEFAULT_PORT,
        },
        get_requests,
        timeout: Some(Duration::from_millis(5_000)),
    };
    let results = http_get(request_test, testcase).unwrap();
    assert_eq!(results.len(), 1);
    let result = results.get(0).unwrap();
    assert_eq!(result.header_result.status_code, 200);
}

fn flexo_test_download_large_file(_path_generator: &mut PathGenerator, testcase: &'static str) {
    // This test case is mainly intended to provoke errors due to various 2GiB or 4GiB limits. For instance,
    // sendfile uses off_t as offset (see man 2 sendfile). off_t can be only 32 bit on some platforms.
    let results = download_large_file(testcase);
    assert_eq!(results.len(), 1);
    let result = results.get(0).unwrap();
    assert_eq!(result.header_result.status_code, 200);
    assert_eq!(result.payload_result.as_ref().unwrap().size, LARGE_FILE_SIZE);
    assert!(!result.header_result.cached);
}

fn flexo_test_download_large_file_cached(_path_generator: &mut PathGenerator, testcase: &'static str) {
    // The intention of this test case is to demonstrate that with large files, no issues occur when the file
    // is served from the cache instead of from a remote mirror.
    download_large_file(testcase);
    let results = download_large_file(testcase);
    assert_eq!(results.len(), 1);
    let result = results.get(0).unwrap();
    assert_eq!(result.header_result.status_code, 200);
    assert_eq!(result.payload_result.as_ref().unwrap().size, LARGE_FILE_SIZE);
    assert!(result.header_result.cached);
}

fn download_large_file(testcase: &'static str) -> Vec<HttpGetResult> {
    let get_requests = vec![
        GetRequest {
            path: LARGE_FILE_REQUEST_PATH.to_owned(),
            client_header: AutoGenerated,
        }
    ];
    let request_test = GetRequestTest {
        conn_addr: ConnAddr {
            host: "flexo-server-fast".to_owned(),
            port: DEFAULT_PORT,
        },
        get_requests,
        timeout: Some(Duration::from_millis(60_000)),
    };
    http_get(request_test, testcase).unwrap()
}

fn flexo_test_download_large_file_cached_resume(_path_generator: &mut PathGenerator, testcase: &'static str) {
    // The resume feature can only be used when the file is already cached, so we download it before continuing
    // with the actual test:
    download_large_file(testcase);
    let start_byte = 6291456;
    let remaining_size = LARGE_FILE_SIZE - start_byte;
    let header = format!("GET {} HTTP/1.1\r\nHost: {}\r\nRange: bytes={}-{}",
                         LARGE_FILE_REQUEST_PATH, "flexo-server-fast", start_byte, HEADER_SEPARATOR_STR);
    let get_requests = vec![
        GetRequest {
            path: LARGE_FILE_REQUEST_PATH.to_owned(),
            client_header: Custom(header),
        }
    ];
    let request_test = GetRequestTest {
        conn_addr: ConnAddr {
            host: "flexo-server-fast".to_owned(),
            port: DEFAULT_PORT,
        },
        get_requests,
        timeout: Some(Duration::from_millis(60_000)),
    };
    let results = http_get(request_test, testcase).unwrap();
    assert_eq!(results.len(), 1);
    let result = results.get(0).unwrap();
    assert_eq!(result.header_result.status_code, 206);
    assert_eq!(result.payload_result.as_ref().unwrap().size, remaining_size);
    assert_eq!(result.header_result.content_length, remaining_size);
}

fn receive_first<T>(receivers: Vec<Receiver<T>>) -> usize  {
    loop {
        for (idx, receiver) in receivers.iter().enumerate() {
            match receiver.recv_timeout(Duration::from_millis(5)) {
                Ok(_) => {
                    return idx;
                }
                Err(_) => {},
            }
        }
    }
}

fn flexo_test_parallel_downloads_nonblocking(path_generator: &mut PathGenerator, testcase: &'static str) {
    let (sender1, receiver1) = mpsc::channel::<Vec<HttpGetResult>>();
    let (sender2, receiver2) = mpsc::channel::<Vec<HttpGetResult>>();
    let host = "flexo-server-slow-primary".to_owned();
    let request_test_1 = GetRequestTest {
        conn_addr: ConnAddr {
            host: host.clone(),
            port: DEFAULT_PORT,
        },
        get_requests: vec![
            GetRequest {
                path: "/zero".to_owned(), // Download a large file
                client_header: AutoGenerated,
            },
        ],
        timeout: None,
    };
    let request_test_2 = GetRequestTest {
        conn_addr: ConnAddr {
            host: host.clone(),
            port: DEFAULT_PORT,
        },
        get_requests: vec![
            GetRequest {
                path: path_generator.generate(), // Download a small file
                client_header: AutoGenerated,
            },
        ],
        timeout: None,
    };
    // Start downloading the large file before downloading the small file.
    std::thread::spawn(move || {
        match http_get(request_test_1, testcase) {
            None => {}
            Some(r) => {
                // Ignore the result: when the 2nd thread was faster, the channel is already closed;
                let _ = sender1.send(r);
            }
        }
    });
    std::thread::spawn(move || {
        match http_get(request_test_2, testcase) {
            None => {}
            Some(r) => {
                // Ignore the result: when the 1st thread was faster, the channel is already closed;
                let _ = sender2.send(r);
            }
        }
    });
    // Even though we started by downloading the large file, the small file was completed first, thus proving
    // that a request does not need to complete before subsequent requests can finish.
    let first_result_idx = receive_first(vec![receiver1, receiver2]);
    assert_eq!(first_result_idx, 1);
}

fn flexo_test_mirror_stalling(path_generator: &mut PathGenerator, testcase: &'static str) {
    let get_requests = vec![
        GetRequest {
            path: path_generator.generate(),
            client_header: AutoGenerated,
        }
    ];
    let request_test = GetRequestTest {
        conn_addr: ConnAddr {
            host: "flexo-server-mirror-stalling".to_owned(),
            port: DEFAULT_PORT,
        },
        get_requests,
        timeout: Some(Duration::from_millis(5_000)),
    };
    let results = http_get(request_test, testcase).unwrap();
    assert_eq!(results.len(), 1);
    let result = results.get(0).unwrap();
    assert_eq!(result.header_result.status_code, 200);
}

fn flexo_test_mirror_stalling_after_header(path_generator: &mut PathGenerator, testcase: &'static str) {
    let get_requests = vec![
        GetRequest {
            path: path_generator.generate(),
            client_header: AutoGenerated,
        }
    ];
    let request_test = GetRequestTest {
        conn_addr: ConnAddr {
            host: "flexo-server-mirror-stalling-after-header".to_owned(),
            port: DEFAULT_PORT,
        },
        get_requests,
        timeout: Some(Duration::from_millis(5_000)),
    };
    let results = http_get(request_test, testcase).unwrap();
    assert_eq!(results.len(), 1);
    let result = results.get(0).unwrap();
    assert_eq!(result.header_result.status_code, 200);
}


fn flexo_test_404_not_found(_path_generator: &mut PathGenerator, testcase: &'static str) {
    let get_requests = vec![
        GetRequest {
            path: "/does-not-exist".to_owned(),
            client_header: AutoGenerated,
        }
    ];
    let request_test = GetRequestTest {
        conn_addr: ConnAddr {
            host: "flexo-server".to_owned(),
            port: DEFAULT_PORT,
        },
        get_requests,
        timeout: None,
    };
    let results = http_get(request_test, testcase).unwrap();
    assert_eq!(results.len(), 1);
    let result = results.get(0).unwrap();
    assert_eq!(result.header_result.status_code, 404);
}

fn flexo_test_root_path_400(_path_generator: &mut PathGenerator, testcase: &'static str) {
    let root_path = "/".to_owned();
    let get_requests = vec![
        GetRequest {
            path: root_path,
            client_header: AutoGenerated,
        }
    ];
    let request_test = GetRequestTest {
        conn_addr: ConnAddr {
            host: "flexo-server".to_owned(),
            port: DEFAULT_PORT,
        },
        get_requests,
        timeout: None,
    };
    let results = http_get(request_test, testcase).unwrap();
    assert_eq!(results.len(), 1);
    let result = results.get(0).unwrap();
    assert_eq!(result.header_result.status_code, 400);
}

fn flexo_test_empty_path_400(_path_generator: &mut PathGenerator, testcase: &'static str) {
    let empty_path = "".to_owned();
    let get_requests = vec![
        GetRequest {
            path: empty_path,
            client_header: AutoGenerated,
        }
    ];
    let request_test = GetRequestTest {
        conn_addr: ConnAddr {
            host: "flexo-server".to_owned(),
            port: DEFAULT_PORT,
        },
        get_requests,
        timeout: None,
    };
    let results = http_get(request_test, testcase).unwrap();
    assert_eq!(results.len(), 1);
    let result = results.get(0).unwrap();
    assert_eq!(result.header_result.status_code, 400);
}

fn flexo_test_no_content_length(_path_generator: &mut PathGenerator, testcase: &'static str) {
    let get_requests = vec![
        GetRequest {
            path: "/test_1".to_owned(),
            client_header: AutoGenerated,
        }
    ];
    let request_test = GetRequestTest {
        conn_addr: ConnAddr {
            host: "flexo-server-no-content-length-primary".to_owned(),
            port: DEFAULT_PORT,
        },
        get_requests,
        timeout: None,
    };
    let results = http_get(request_test, testcase).unwrap();
    assert_eq!(results.len(), 1);
    let result = results.get(0).unwrap();
    assert_eq!(result.header_result.status_code, 200);
}

fn flexo_test_redirect(_path_generator: &mut PathGenerator, testcase: &'static str) {
    // Regression test: Flexo used to be unable to handle a mirror that replies with several redirects.
    let get_requests = vec![
        GetRequest {
            path: "/redirect_1/test_1".to_owned(),
            client_header: AutoGenerated,
        }
    ];
    let request_test = GetRequestTest {
        conn_addr: ConnAddr {
            host: "flexo-server-redirect".to_owned(),
            port: DEFAULT_PORT,
        },
        get_requests,
        timeout: None,
    };
    let results = http_get(request_test, testcase).unwrap();
    assert_eq!(results.len(), 1);
    let result = results.get(0).unwrap();
    assert_eq!(result.header_result.status_code, 200);
}

fn flexo_test_file_partially_cached(_path_generator: &mut PathGenerator, testcase: &'static str) {
    // The requested file is available in the cache, but only partially: In this case, the file has a size of 100 bytes,
    // and the first 50 bytes are already stored in the cache.
    let get_requests = vec![
        GetRequest {
            path: FILE_PARTIALLY_CACHED_REQUEST_PATH.to_owned(),
            client_header: AutoGenerated,
        }
    ];
    let request_test = GetRequestTest {
        conn_addr: ConnAddr {
            host: "flexo-server-fast".to_owned(),
            port: DEFAULT_PORT,
        },
        get_requests,
        timeout: None,
    };
    let results = http_get(request_test, testcase).unwrap();
    assert_eq!(results.len(), 1);
    let result = results.get(0).unwrap();
    assert_eq!(result.header_result.status_code, 200);
    assert_eq!(result.payload_result.as_ref().unwrap().size, FILE_PARTIALLY_CACHED_SIZE);
    assert!(!result.header_result.cached);
}

fn flexo_test_parallel_downloads_same_file(_path_generator: &mut PathGenerator, testcase: &'static str) {
    // Download the same file from 2 clients, such that Flexo needs to handle 2 downloads simultaneously while
    // downloading one file from the remote mirror.
    let (sender1, receiver1) = mpsc::channel::<Vec<HttpGetResult>>();
    let (sender2, receiver2) = mpsc::channel::<Vec<HttpGetResult>>();
    let host = "flexo-server-medium-bandwidth".to_owned();
    let request_test_1 = GetRequestTest {
        conn_addr: ConnAddr {
            host: host.clone(),
            port: DEFAULT_PORT,
        },
        get_requests: vec![
            GetRequest {
                path: "/random_small".to_owned(),
                client_header: AutoGenerated,
            },
        ],
        timeout: Some(Duration::from_secs(300)),
    };
    let request_test_2 = request_test_1.clone();
    // Start downloading the large file before downloading the small file.
    std::thread::spawn(move || {
        match http_get(request_test_1, testcase) {
            None => {}
            Some(r) => {
                sender1.send(r).unwrap();
            }
        }
    });
    std::thread::sleep(Duration::from_secs(1));
    std::thread::spawn(move || {
        match http_get(request_test_2, testcase) {
            None => {}
            Some(r) => {
                sender2.send(r).unwrap();
            }
        }
    });
    let results1 = receiver1.recv().unwrap();
    let results2 = receiver2.recv().unwrap();
    assert_eq!(results1.len(), 1);
    assert_eq!(results2.len(), 1);
    let result1 = results1.get(0).unwrap();
    let result2 = results2.get(0).unwrap();
    assert_eq!(result1.header_result.status_code, 200);
    assert_eq!(result2.header_result.status_code, 200);
    assert_eq!(result1.payload_result.as_ref().unwrap().size, FILE_ZERO_SMALL_SIZE);
    assert_eq!(result2.payload_result.as_ref().unwrap().size, FILE_ZERO_SMALL_SIZE);
    let sha1: String = hex::encode(result1.payload_result.as_ref().unwrap().clone().sha);
    let sha2: String = hex::encode(result2.payload_result.as_ref().unwrap().clone().sha);
    let expected_sha = "a1eafe1d59f7d2cec131064fc844e806067b713c0639f12593860fce137bfd46".to_string();
    assert_eq!(sha1, expected_sha);
    assert_eq!(sha2, expected_sha);
}