use serde::Serialize;
use std::{env, fs, path::Path};
use tantivy::collector::Count;

use std::io::prelude::*;
use std::io::{BufRead, BufReader, LineWriter};
use std::os::unix::net::{UnixListener, UnixStream};
use std::thread;
mod config;
mod search;
use search::QuerySchema;
mod migrate;
mod tokenizer;

static DEFAULT_MAX_SIZE: usize = 8;

#[derive(Serialize)]
struct Hit {
    url: String,
    date: i64,
    title: String,
    snippet: String,
}

#[derive(Serialize)]
struct Err {
    err_msg: String,
}

#[derive(Serialize)]
struct Response {
    count: usize,
    data: Vec<Hit>,
}

fn execute(pages: &str, terms: &str, q: &str, range: &str, query_schema: &QuerySchema) -> String {
    let kq = query_schema.make_keyword_query(q);
    if kq.is_err() {
        return format!("{{\"err_msg\": \"{}\"}}\n", kq.unwrap_err().to_string());
    }
    let mut box_qs = kq.unwrap();
    query_schema.make_terms_query(terms, &mut box_qs);
    query_schema.make_date_query(range, &mut box_qs);
    if box_qs.len() == 0 {
        return format!("{{\"err_msg\": \"It is forbidden queries that are empty.\"}}\n");
    }
    let content_gen = query_schema.make_snippet_gen(&box_qs[0], query_schema.fields.content);
    let title_gen = query_schema.make_snippet_gen(&box_qs[0], query_schema.fields.title);

    let bool_qs = query_schema.make_bool_query(box_qs);
    let searcher = query_schema.reader.searcher();

    let (top_docs, num) = searcher
        .search(&bool_qs, &(query_schema.make_paginate(pages), Count))
        .expect("Search Failed");
    let mut results: Vec<Hit> = Vec::with_capacity(DEFAULT_MAX_SIZE);
    for (_score, doc_addr) in top_docs {
        let doc = searcher.doc(doc_addr).expect("Not Found Document Address");
        let values = doc.get_sorted_field_values();
        let title = query_schema.make_snippet_value(&title_gen, &doc, values[0].1[0].value());
        let snippet = query_schema.make_snippet_value(&content_gen, &doc, values[1].1[0].value());
        results.push(Hit {
            url: values[3].1[0].value().text().expect("Err Url").to_string(),
            date: values[2].1[0].value().i64_value().expect("Err date"),
            title,
            snippet,
        });
    }
    let se_result = serde_json::json!(Response {
        count: num,
        data: results,
    });
    se_result.to_string() + "\n"
}

fn handle_client(stream: UnixStream, qs: QuerySchema) {
    println!("new client: {:?}", stream);
    let stream_reader = BufReader::new(&stream);
    let mut stream_writer = LineWriter::new(&stream);
    for line in stream_reader.lines() {
        let params = line.expect("error read from stream");
        let v: Vec<&str> = params.split(0 as char).collect();
        {
            let result = execute(v[0], v[1], v[2], v[3], &qs);
            match stream_writer.write_all(result.as_bytes()) {
                Ok(_) => (),
                Err(err) => println!("{:?}", err),
            };
        }
    }
    println!("closed connection. {:?}", stream);
}

fn loop_accept(socket_path: &str, qs: QuerySchema) {
    if Path::new(socket_path).exists() {
        fs::remove_file(socket_path).unwrap();
    }
    println!("Listening on file {:}", socket_path);
    let listener = UnixListener::bind(socket_path).expect("Binding to file error");
    for stream in listener.incoming() {
        let tmp = qs.clone();
        match stream {
            Ok(stream) => {
                thread::spawn(move || handle_client(stream, tmp));
            }
            Err(err) => {
                println!("Error: {}", err);
                break;
            }
        }
    }
}

fn main() {
    let args = env::args().collect::<Vec<String>>();
    if args.len() == 1 {
        println!(
            "Run with one argument: 
        1. init (Initial tanitvy schema and database. this will empty exists directory.)
        2. migrate (Append new article to exists directory.)
        3. run (run server with unix domain socket.)"
        );
        return;
    }
    let config = config::read_config();
    match args[1].as_ref() {
        "init" => {
            migrate::create_dir(&config.tantivy_db);
            migrate::init_schema(&config.tantivy_db, &config.blog_source);
        }
        "migrate" => {
            migrate::init_schema(&config.tantivy_db, &config.blog_source);
        }
        "run" => {
            let qs = QuerySchema::new(&config.tantivy_db);
            loop_accept(&config.listen_addr, qs);
        }
        _ => (),
    }
}
