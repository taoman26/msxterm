// MSX Term
// Copyright (c) 2023 Akio Setsumasa 
// Released under the MIT license
// https://github.com/akio-se/msxterm
//

// 初期開発中は Warnning 抑制
#![allow(unused_variables)]
#![allow(dead_code)]
//
mod msxcode;
mod connection;

use std::net::{Shutdown, TcpStream};
use std::thread;

use rustyline::config::Configurer;
use std::sync::{Arc, Mutex};
use rustyline::{DefaultEditor, EditMode, ExternalPrinter, Result, error::ReadlineError};
use std::collections::{BTreeMap, HashMap};
use clap::Parser;
use std::fs::File;
use std::io::{BufRead, Write, BufReader,BufWriter};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};
//use serialport::{ SerialPort, SerialPortType, available_ports};
use serial2::{SerialPort};
//use crate::connection::{TcpConnection, SerialConnection};

const C_CR: char = '\u{000d}';
const C_LF: char = '\u{000a}';

const U_BREAK:u8 = 0x03;
const U_BS:u8 = 0x08;
const U_LF:u8 = 0x0a;
const U_CR:u8 = 0x0d;
const U_PAUSE:u8 = 0x7b;

fn dump_hex(uv: Vec<u8>) -> String
{
    let mut cv:String = "".to_string();
    for u in uv {
        let tmp = format!("{:02X} ", u);
        cv.push_str(tmp.as_str());
    }
    cv
}

#[test]
fn test_dump_hex() {
    let uv: Vec<u8> = [0x41,0x51,0x61,0x71,0x80,0x81,0x8A,0xB3,0xC4,0x55].to_vec();
    let s = dump_hex(uv);
    println!("{}",s);
    assert!(s == "41 51 61 71 80 81 8A B3 C4 55 ");
}


#[test]
fn test_hex () {
    let s = "#HEX 40 41 42 43 44";
    let v = hex2u8(s);
    println!("{:?}", v);
}

fn hex2u8(hex: &str) -> Vec<u8> {
    let mut hex_vec: Vec<u8> = Vec::new();
    let tokens: Vec<&str> = hex.split(' ').collect();
    for token in &tokens[1..] {
        if let Ok(val) = u8::from_str_radix(token, 16) {
            hex_vec.push(val)   
        }
    }
    hex_vec
}

//
// コマンド行の引数（ファイルパス）を取り出す
// 空白を含むパスはダブルクォートで囲む。引数が無ければ None
//
fn parse_path_arg(command_line: &str) -> Option<String> {
    let rest = command_line.trim().splitn(2, char::is_whitespace).nth(1)?.trim();
    if let Some(quoted) = rest.strip_prefix('"') {
        let end = quoted.find('"').unwrap_or(quoted.len());
        let path = &quoted[..end];
        return if path.is_empty() { None } else { Some(path.to_string()) };
    }
    rest.split_whitespace().next().map(|p| p.to_string())
}

//
// 指定されたファイルをロードしてvec<String>を返す
//
fn load(command_line: &str) -> Result<Vec<String>> {
    let path = parse_path_arg(command_line).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "Usage: #load <file>")
    })?;
    let file = File::open(path)?;
    let reader = BufReader::new(file);       
    let mut lines = Vec::new();
    for line in reader.lines() {
        lines.push(line?);
    }
    Ok(lines)
}

//
// コマンドラインオプションの設定
//
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Host_IP or Serial_Port
    target: Option<String>,

    /// history file name
    #[arg(short, long, value_name = "history file", default_value = "history.txt")]
    file: String,

    #[arg(short, long, value_name= "emacs or vi", default_value = "emacs")]
    editor: Option<String>,

    /// Use Serial Port
    #[arg(short, long)]
    serial: bool,

    /// Display Serial Port List
    #[arg(short, long)]
    port_list: bool,
}

struct Msxterm {
    dump_mode: bool,
    lower_mode: bool,
    kanji_mode: bool,
    prog_buff:BTreeMap<u16, String>,
    t_com: HashMap<String, String>,
}

impl Msxterm {
    pub fn new() -> Msxterm {
        Msxterm { 
            dump_mode: false, 
            lower_mode: false,
            kanji_mode: false,
            prog_buff: BTreeMap::new(), 
            t_com: HashMap::new(),
        }
    }
    fn init(&mut self) {
        self.t_com.insert("#list".to_string(), "list Program".to_string());
    }

    pub fn parse_basic(&mut self, line:&str) {
        let mut iter = line.splitn(2,' ');
        if let Some(number) = iter.next() {
            if let Ok(number) = number.parse::<u16>() {
                if let Some(instruction) = iter.next() {
                    self.prog_buff.insert(number, instruction.trim().to_owned());
                } else {
                    self.prog_buff.remove(&number);
                }
            }
        }
    }

    pub fn print_basic(&mut self, start:u16, end:u16) -> Vec<String> {
        // println!("list {} {}", start, end);
        let mut history = Vec::new(); 
        if let Some(maxline) = self.prog_buff.iter().max() {
            let maxlen = maxline.0.to_string().len();
            let iter = self.prog_buff.range(start..=end);
            //for (num, inst) in &self.prog_buff {
            for (num ,inst) in iter {
                let padding = " ".repeat(maxlen - num.to_string().len());
                println!("{}\x1b[36m{}\x1b[0m {}",padding, num, inst);
                history.push(std::format!("{} {}", num, inst ));
            }
        }
        history
    }

    pub fn clear_basic(&mut self) {
        self.prog_buff.clear();
    }

    pub fn save_program(&self, command_line: &str) -> std::result::Result<usize, String> {
        let path = parse_path_arg(command_line).ok_or("Usage: #save <file>")?;
        if self.prog_buff.is_empty() {
            return Err("Program buffer is empty. Nothing saved. (use #reload_from to fetch the program from MSX0)".to_string());
        }
        let file = File::create(&path).map_err(|e| format!("{}: {}", path, e))?;
        let mut writer = BufWriter::new(file);
        for (line_number, program) in self.prog_buff.iter() {
            let line = format!("{} {}\n", line_number, program);
            writer.write_all(line.as_bytes()).map_err(|e| format!("{}: {}", path, e))?;
        }
        writer.flush().map_err(|e| format!("{}: {}", path, e))?;
        Ok(self.prog_buff.len())
    }

}

pub fn parse_command(command: &str) -> (Option<u16>, Option<u16>) {
    let mut parts = command.trim().split(' ');
    let _ = parts.next(); // Skip the command name
    let range = parts.next();
    match range {
        None => (None, None),
        Some(range) => {
            let mut range_parts = range.split('-');
            let start = range_parts.next().and_then(|x| x.parse().ok());
            let end = range_parts.next().and_then(|x| x.parse().ok());
            (start, end)
        }
    }
}

// #reload_from 用: 受信スレッドが list の出力を取り込むための共有領域
struct Capture {
    lines: Vec<String>,
    done: bool,
    last_rx: Instant,
}
type SharedCapture = Arc<Mutex<Option<Capture>>>;

// MSX0 に list を送り、行番号付きの行を集めて返す
fn fetch_program(stream: &mut TcpStream, capture: &SharedCapture) -> std::result::Result<Vec<String>, String> {
    *capture.lock().unwrap() = Some(Capture { lines: Vec::new(), done: false, last_rx: Instant::now() });
    if let Err(e) = stream.write_all(&[b'l', b'i', b's', b't', C_CR as u8]) {
        capture.lock().unwrap().take();
        return Err(format!("Failed to write to server: {}", e));
    }
    let start = Instant::now();
    let timed_out = loop {
        thread::sleep(Duration::from_millis(20));
        let guard = capture.lock().unwrap();
        let cap = guard.as_ref().unwrap();
        if cap.done {
            break false;
        }
        if cap.lines.is_empty() && start.elapsed() > Duration::from_secs(5) {
            break true;
        }
        if !cap.lines.is_empty() && cap.last_rx.elapsed() > Duration::from_secs(1) {
            break false;
        }
    };
    let cap = capture.lock().unwrap().take().unwrap();
    if timed_out {
        return Err("No response from MSX0 (timeout). Program buffer is unchanged.".to_string());
    }
    Ok(cap.lines)
}

enum Command {
    DumpModeOn,
    DumpModeOff,
    KanjiModeOn,
    KanjiModeOff,
}

#[test]
fn test_msxterm () {
    let mut mt = Msxterm::new();
    mt.init();

    let basfile = load("#load ./src/test.bas").unwrap();
    for s in basfile {
        mt.parse_basic(&s);
    }
    mt.print_basic(0,65530);

    let (st,ed) = parse_command("#list 10-20");
    println!("list {}-{}", st.unwrap_or(0), ed.unwrap_or(65530));

    let (st,ed) = parse_command("#list 40-");
    println!("list {}-{}", st.unwrap_or(0), ed.unwrap_or(65530));

    let (st,ed) = parse_command("#list -50");
    println!("list {}-{}", st.unwrap_or(0), ed.unwrap_or(65530));

    let (st,ed) = parse_command("#list 50");
    println!("list {}-{}", st.unwrap_or(0), ed.unwrap_or(65530));

    /*
    mt.parse_basic("1000 print 10 + 20");
    mt.parse_basic("100 cls");
    mt.parse_basic("1010 goto 1000");
    mt.parse_basic("20010 gosub 1000");
    mt.print_basic(0,65530);
    mt.parse_basic("100");
    mt.parse_basic("#list");
    mt.print_basic(0,65530);

    mt.parse_basic("1010 for i=0 to 100");
    mt.parse_basic("1020 PRINT I");
    mt.parse_basic("1030 NEXT I");
    mt.parse_basic("20010 gosub 1000");
    mt.print_basic(2000, 65530);
    */
}

fn lower_program(input:&str) -> String {
    let mut output = String::new();
    let mut is_quoted = false;

    for line in input.lines() {
        let mut tmp="".to_string();
        let tline = line.trim();
        for c in tline.chars() { 
            if c == '"' {
                is_quoted = !is_quoted;
            }
            if is_quoted {
                tmp.push(c);
            } else {
                let lowercase = c.to_lowercase().next().unwrap();
                tmp.push(lowercase);
            }
        }
        if tmp.starts_with("rem") {
            output.push_str(line.trim());
            output.push(C_CR);
        } else {
            output.push_str(&tmp);
            output.push(C_CR);
        }
    }
    output
}

#[test]
fn test_lower_program() {
    let text = "input text \"PrintHello\"
                        REM Akio Setsumasa
                        Print A$ + B$
                        REM This Program is Free
                      END";
    println!("{}", text);
    let result = lower_program(text);
    println!("{}", result);
}

fn serial_port_list() {
    // シリアルポートの情報を取得する
    let ports = SerialPort::available_ports().expect("Failed to get serial port list");

    // USB接続されたシリアルポートを検索する
    for port in ports {
        let str = port.into_os_string().into_string().unwrap();
        println!("USB Serial Port found: {}", str);
    }
}


fn main() -> Result<()> {
    // 変数初期化
    let mut msxterm = Msxterm::new();
    msxterm.init();

    // コマンドライン引数取得
    let args = Args::parse();
    if args.port_list {
        serial_port_list();
        return Ok(());
    }
    let target = match args.target {
        Some(target) => {
            target
        },
        _ => {
             "".to_string()    
         }
    };
/*
    println!("file {}", args.file);
    println!("serial {}", args.serial);
    println!("portlist {}", args.port_list);
*/
    // エディタを生成
    let mut rl = DefaultEditor::new()?;
    match args.editor {
        Some(ed) =>  {
            if ed.eq("emacs") {
                rl.set_edit_mode(EditMode::Emacs);
            } else if ed.eq("vi") {
                rl.set_edit_mode(EditMode::Vi);
            }
        },
        _ => {}
    }

    let mut printer = rl.create_external_printer()?;
    if rl.load_history(&args.file).is_err() {
        println!("No previous history.");
    }

    // ソケットを接続
    let server_address = target.clone();
    println!("Connecting... {}", server_address);

/*
    let mut conn = connection::create_connection(&target);
    let mut conn_read  = Arc::new(Mutec::new(conn));
    let mut conn_write = Arc::new(Mutec::new(conn));
*/

    let mut stream;
    let r = TcpStream::connect(server_address);
    match r {
        Ok(s) => {
            stream = s;
            println!("connected.");
        },
        Err(_) => {
            eprintln!("Failed to connect.");
            return Ok(());
        }, 
    }

    // 通信スレッドとメインスレッド間でやりとりするチャンネルを作成する
    let (tx, rx): (Sender<Command>, Receiver<Command>) = channel();

    // #reload_from 用の共有領域
    let capture: SharedCapture = Arc::new(Mutex::new(None));
    let capture_rx = Arc::clone(&capture);

    // 受信用スレッドを作成
    let stream_clone = stream.try_clone().expect("Failed to clone stream");
    let receive_thread = thread::spawn(move || {
        let mut dump_mode = false;
        let mut kanji_mode = false;
        let mut reader = std::io::BufReader::new(&stream_clone);
        loop {
            if let Ok(command) = rx.recv_timeout(Duration::from_millis(1)) {
                match command {
                    Command::DumpModeOn => dump_mode = true,
                    Command::DumpModeOff => dump_mode = false,
                    Command::KanjiModeOn => kanji_mode = true,
                    Command::KanjiModeOff => kanji_mode = false,
                }
            }
            let mut byte_buff: Vec<u8> = [0x00_u8; 0].to_vec();
            let result = reader.read_until(U_LF, &mut byte_buff);
            match result {
                Ok(size) => {
                    if size == 0 {
                        printer.print("Tcp disconnect".to_string()).expect("External print failure");
                        break;
                    }    
                },
                Err(e) => {
                    printer.print(e.to_string()).expect("External print failure");
                    break;
                }
            }
            if let Ok(mut guard) = capture_rx.lock() {
                if let Some(cap) = guard.as_mut() {
                    let text = if kanji_mode {
                        msxcode::msx_kanji_to_string(byte_buff)
                    } else {
                        msxcode::msx_ascii_to_string(byte_buff)
                    };
                    let text = text.trim();
                    cap.last_rx = Instant::now();
                    if text == "Ok" {
                        cap.done = true;
                    } else if text.starts_with(|c: char| c.is_ascii_digit()) {
                        cap.lines.push(text.to_string());
                    }
                    continue;
                }
            }
            if dump_mode {
                let recv_buff = dump_hex(byte_buff);
                printer.print(recv_buff).expect("External print failure");
            } else {
                let recv_buff = if kanji_mode {
                    msxcode::msx_kanji_to_string(byte_buff)
                } else {
                    msxcode::msx_ascii_to_string(byte_buff)
                };
                printer.print(recv_buff).expect("External print failure");    
            }
        }
    });

    // エディタ入力とコマンド送信のメインループ
    'input:loop {
        let readline = rl.readline("> ");
        match readline {
            Ok(tmpl) => {
                //let mut line_tmp: &str = line.as_str();
                let b = tmpl.as_str().replace("\r\n","\r").replace('\n',"\r");
                let lines: Vec<&str> = b.split(C_CR).collect();
                for line in lines {
                    rl.add_history_entry(line)?;

                    if line.starts_with("#quit") {
                        // TCP 接続終了
                        stream.shutdown(Shutdown::Both).expect("Shutdown Error");
                        break 'input;
                    }
                    if line.starts_with("#hex") {
                        let hex = hex2u8(line);
                        stream.write(&hex).expect("Failed to write to server");
                        continue;
                    }
                    if line.starts_with("#dump_on") {
                        tx.send(Command::DumpModeOn).expect("Thread sync Error");
                        println!("Output dump mode On");
                        continue;             
                    }
                    if line.starts_with("#dump_off") {
                        tx.send(Command::DumpModeOff).expect("Thread sync Error");
                        println!("Output dump mode Off");
                        continue;             
                    }
                    if line.starts_with("#lowsend_on") {
                        msxterm.lower_mode = true;
                        println!("Lower Case send mode On");
                        continue;
                    }
                    if line.starts_with("#lowsend_off") {
                        msxterm.lower_mode = false;
                        println!("Lower Case send mode Off");
                        continue;
                    }
                    if line.starts_with("#kanji_on") {
                        tx.send(Command::KanjiModeOn).expect("Thread sync Error");
                        msxterm.kanji_mode = true;
                        println!("Kanji mode On");
                        continue;                        
                    }
                    if line.starts_with("#kanji_off") {
                        tx.send(Command::KanjiModeOff).expect("Thread sync Error");
                        msxterm.kanji_mode = false;
                        println!("Kanji mode Off");
                        continue;
                    }
                    if line.starts_with("#emacs") {
                        rl.set_edit_mode(EditMode::Emacs);
                        continue;
                    }
                    if line.starts_with("#vi") {
                        rl.set_edit_mode(EditMode::Vi);
                        continue;
                    }
                    if line.starts_with("#clear_history") {
                        rl.clear_history().unwrap();
                        println!("History is cleared.");
                        continue;
                    }
                    if line.starts_with("#new") {
                        msxterm.prog_buff.clear();
                        println!("Program Buffer is cleared.");
                        continue;
                    }
                    if line.starts_with("#load") {
                        match load(line) {
                            Ok(basic) => {
                                let mut ld_program = "".to_string();
                                for bl in basic {
                                    let mut tmp = bl.trim().to_string();
                                    msxterm.parse_basic(tmp.as_str());
                                    rl.add_history_entry(tmp.as_str())?;
                                    tmp.push(C_CR);
                                    ld_program.push_str(&tmp);
                                }
                                if msxterm.lower_mode {
                                    ld_program = lower_program(&ld_program);
                                }
                                stream
                                .write(ld_program.as_bytes())
                                .expect("Failed to write to server");
                                println!("Ok");
                            },
                            Err(e) => {
                                println!("{}", e);
                            }
                        }
                        continue;
                    }
                    if line.starts_with("#list") {
                        let cols = line.split(' ');
                        for history in msxterm.print_basic(0, 65530) {
                            rl.add_history_entry(history)?;
                        }
                        continue;
                    }
                    if line.starts_with("#save") {
                        match msxterm.save_program(line) {
                            Ok(n) => println!("Ok ({} lines saved)", n),
                            Err(e) => println!("{}", e),
                        }
                        continue;
                    }
                    if line.starts_with("#reload_from") {
                        match fetch_program(&mut stream, &capture) {
                            Ok(lines) => {
                                msxterm.prog_buff.clear();
                                for l in &lines {
                                    msxterm.parse_basic(l);
                                }
                                println!("Ok ({} lines loaded from MSX0)", msxterm.prog_buff.len());
                            },
                            Err(e) => println!("{}", e),
                        }
                        continue;
                    }

                    msxterm.parse_basic(line);

                    let mut tmp2 = line.to_string();
                    tmp2.push(C_CR);
                    if msxterm.lower_mode {
                        tmp2 = lower_program(&tmp2);
                    }

                    let faces_code = if msxterm.kanji_mode {
                        msxcode::utf8_to_msx_kanji(tmp2.as_str())
                    } else {
                        msxcode::utf8_msx_jp_code(tmp2.as_str())
                    };

                    stream
                        .write(&faces_code).expect("Failed to write");
                }
            }
            Err(ReadlineError::Interrupted) => {
                // break 送信
                let buf = vec![U_BREAK];
                stream.write(&buf).expect("Failed to write");
                continue;
            }
            Err(ReadlineError::Eof) => {
                // BS 送信
                let buf = vec![U_PAUSE];
                stream.write(&buf).expect("Failed to write");
                continue;
            }
            Err(err) => {
                println!("Error: {err:?}");
                break;
            }
        }
    }
    // 受信スレッド終了
    receive_thread
        .join()
        .expect("Failed to join receive thread");

    // 履歴ファイル記録
    match rl.save_history(& args.file) 
    {
        Ok(_) => {
            println!("history save to {}", args.file);
        },
        Err(e) => {
            println!("{}", e.to_string());
        }
    }
    Ok(())
}

#[test]
fn test_parse_path_arg() {
    assert_eq!(parse_path_arg("#save"), None);
    assert_eq!(parse_path_arg("#save "), None);
    assert_eq!(parse_path_arg("#save \"\""), None);
    assert_eq!(parse_path_arg("#save a.bas"), Some("a.bas".to_string()));
    assert_eq!(parse_path_arg("#save  a.bas"), Some("a.bas".to_string()));
    assert_eq!(parse_path_arg("#save \"with space.bas\""), Some("with space.bas".to_string()));
    assert_eq!(parse_path_arg("#load \"c:\\my file name\""), Some("c:\\my file name".to_string()));
}

#[test]
fn test_save_program_errors() {
    let mt = Msxterm::new();
    assert!(mt.save_program("#save").is_err());
    assert!(mt.save_program("#save /nonexistent_dir/x.bas").is_err());
    let mut mt = Msxterm::new();
    mt.parse_basic("10 PRINT 1");
    assert!(mt.save_program("#save").is_err());
    assert!(mt.save_program("#save /nonexistent_dir/x.bas").is_err());
}
