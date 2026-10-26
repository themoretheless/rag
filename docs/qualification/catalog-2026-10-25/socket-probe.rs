use std::{io::{Read,Write},net::{TcpListener,TcpStream},thread,time::{Duration,Instant}};
fn probe(fixed:bool)->(bool,u128,String){
 let listener=TcpListener::bind("127.0.0.1:0").unwrap(); listener.set_nonblocking(true).unwrap();let addr=listener.local_addr().unwrap();
 let client=thread::spawn(move||{let mut s=TcpStream::connect(addr).unwrap();thread::sleep(Duration::from_millis(100));let _=s.write_all(b"GET / HTTP/1.1\r\n\r\n");});
 let mut stream=loop{match listener.accept(){Ok((s,_))=>break s,Err(e) if e.kind()==std::io::ErrorKind::WouldBlock=>thread::sleep(Duration::from_millis(1)),Err(e)=>panic!("{e}")}};
 if fixed{stream.set_nonblocking(false).unwrap();}stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();let started=Instant::now();let mut buf=[0u8;256];let result=stream.read(&mut buf);let elapsed=started.elapsed().as_millis();client.join().unwrap();(result.as_ref().is_ok_and(|n|*n>0),elapsed,format!("{result:?}"))
}
fn main(){let before=probe(false);let after=probe(true);println!("{{\"before_success\":{},\"before_elapsed_ms\":{},\"before_result\":{:?},\"after_success\":{},\"after_elapsed_ms\":{},\"after_result\":{:?}}}",before.0,before.1,before.2,after.0,after.1,after.2);assert!(after.0);}
