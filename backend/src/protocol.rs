//! Compact newline-delimited JSON-RPC. Stdout is exclusively protocol output.
use crate::{discovery, snapshot};
use json::{object, JsonValue};
use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
};

pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

/// Reserve a complete maximum-size request for the echoed ID (including any
/// numeric spelling), plus the operation outcome envelope and newline. Counting
/// uses the same serializer as the wire, including UTF-8 and JSON escaping.
/// No response-sized allocation is needed for this preflight.
pub const NATIVE_SNAPSHOT_BUDGET: usize = MAX_RESPONSE_BYTES - MAX_REQUEST_BYTES - 256;

pub fn native_value_bytes(value: &JsonValue) -> Option<usize> {
    struct Counter(usize);
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.0 {
                return Err(io::Error::new(
                    io::ErrorKind::FileTooLarge,
                    "response limit",
                ));
            }
            self.0 -= bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    // 256 covers {jsonrpc,id,result:{operationId:<64 safe ASCII>,
    // status:"succeeded",result:<snapshot>}} and the trailing newline.
    let mut counter = Counter(NATIVE_SNAPSHOT_BUDGET);
    value.write(&mut counter).ok()?;
    Some(NATIVE_SNAPSHOT_BUDGET - counter.0)
}

fn error(id: JsonValue, code: i32, kind: &str, message: &str) -> JsonValue {
    object! {jsonrpc:"2.0",id:id,error:object!{code:code,message:message,data:object!{kind:kind}}}
}
fn invalid_params(id: JsonValue) -> JsonValue {
    error(
        id,
        -32602,
        "invalidParams",
        "Params must be an object; statePath and profileId must be strings when provided.",
    )
}

/// Validate the numeric value without float rounding or the dependency's
/// signed conversion (which overflows at i64::MIN in debug builds).
pub(crate) fn signed_integer_id(value: &JsonValue) -> bool {
    let Some(number) = value.as_number() else {
        return false;
    };
    let (positive, mantissa, exponent) = number.as_parts();
    if mantissa == 0 {
        return true;
    }
    let magnitude = if exponent >= 0 {
        10u64
            .checked_pow(exponent as u32)
            .and_then(|scale| mantissa.checked_mul(scale))
    } else {
        10u64
            .checked_pow(exponent.unsigned_abs() as u32)
            .and_then(|scale| {
                if mantissa % scale == 0 {
                    Some(mantissa / scale)
                } else {
                    None
                }
            })
    };
    let limit = if positive {
        i64::MAX as u64
    } else {
        1u64 << 63
    };
    magnitude.is_some_and(|magnitude| magnitude <= limit)
}

fn dispatch(native: &mut crate::native::NativeService, bytes: &[u8]) -> Option<JsonValue> {
    let request = std::str::from_utf8(bytes)
        .ok()
        .and_then(|s| json::parse(s).ok());
    let Some(request) = request else {
        return Some(error(
            JsonValue::Null,
            -32700,
            "parseError",
            "Request is not valid UTF-8 JSON.",
        ));
    };
    let id = &request["id"];
    let valid_id = !request.has_key("id") || signed_integer_id(id);
    if !request.is_object()
        || request["jsonrpc"] != "2.0"
        || !request["method"].is_string()
        || !valid_id
    {
        let id = if valid_id {
            id.clone()
        } else {
            JsonValue::Null
        };
        return Some(error(
            id,
            -32600,
            "invalidRequest",
            "Expected a JSON-RPC 2.0 object with a method and a signed 64-bit integer request ID.",
        ));
    }
    let launch = request["method"].as_str().is_some_and(|m|m.starts_with("native.launch.") || m.starts_with("native.game."));
    if launch && (bytes.len() >= MAX_REQUEST_BYTES || request.len() != 4
        || ["jsonrpc", "id", "method", "params"].iter().any(|k|!request.has_key(k))
        || id.dump().len() > 21 || !unique_keys(bytes)) {
        return Some(error(id.clone(), -32600, "invalidRequest", "Launch controls require an exact bounded JSON-RPC envelope with unique object keys."));
    }
    let catalog = request["method"].as_str().is_some_and(|m|m.starts_with("native.catalog."));
    if catalog && (bytes.len() >= MAX_REQUEST_BYTES || request.len() != 4
        || ["jsonrpc", "id", "method", "params"].iter().any(|k|!request.has_key(k))
        || !request["params"].is_object() || id.dump().len() > 20
        || !unique_keys(bytes) || !integer_numbers(bytes)) {
        return Some(error(id.clone(), -32600, "invalidRequest", "Catalog requires an exact bounded JSON-RPC envelope with unique keys and typed integers."));
    }
    let bootstrap = request["method"].as_str().is_some_and(|m|m.starts_with("native.library."));
    if bootstrap && (bytes.len() >= MAX_REQUEST_BYTES || request.len()!=4
        || ["jsonrpc","id","method","params"].iter().any(|k|!request.has_key(k))
        || !request["params"].is_object() || !unique_keys(bytes) || !integer_numbers(bytes)) {
        return Some(error(id.clone(), -32600, "invalidRequest", "Library bootstrap requires an exact bounded JSON-RPC envelope with unique keys and typed integers."));
    }
    let remote = request["method"].as_str().is_some_and(|m|m.starts_with("native.remote."));
    let recovery_inspect = request["method"]=="native.recovery.inspect";
    if (remote || recovery_inspect) && (bytes.len().checked_add(1).is_none_or(|n|n>MAX_REQUEST_BYTES) || request.len()!=4
        || ["jsonrpc","id","method","params"].iter().any(|k|!request.has_key(k))
        || !request["params"].is_object() || id.dump().len()>20 || !unique_keys(bytes) || !integer_numbers(bytes)) {
        return Some(error(id.clone(),-32600,"invalidRequest","Remote controls require an exact bounded JSON-RPC envelope with unique keys and typed integers."));
    }
    if remote && crate::remote::contract::METHODS.iter().any(|(m,_)|request["method"]==*m)
        && crate::remote::contract::validate_request(request["method"].as_str().unwrap(),&request["params"]).is_err() {
        if matches!(request["method"].as_str(),Some("native.remote.review"|"native.remote.replaceReview")) { native.revoke_remote_review_attempt(&request["params"]); }
        return Some(invalid_params(id.clone()));
    }
    // Notifications never execute native mutations or expensive discarded reads.
    // Avoid expensive snapshots whose result would be discarded.
    if !request.has_key("id") {
        return None;
    }
    let id = id.clone();
    let result = match request["method"].as_str().unwrap() {
        "hello" => {
            if request.has_key("params") && !request["params"].is_object() {
                return Some(invalid_params(id));
            }
            object! {protocolVersion:1,readOnly:false,capabilities:JsonValue::from(crate::native::CAPABILITIES.to_vec())}
        }
        "library.snapshot" => {
            let params = &request["params"];
            if request.has_key("params") && !params.is_object() {
                return Some(invalid_params(id));
            }
            for key in ["statePath", "profileId"] {
                if params.has_key(key) && !params[key].is_string() {
                    return Some(invalid_params(id));
                }
            }
            let path = match params["statePath"].as_str() {
                Some(path) => Ok(PathBuf::from(path)),
                None => discovery::discover(),
            };
            match path.and_then(|path| snapshot::read_snapshot(&path, params["profileId"].as_str()))
            {
                Ok(result) => result,
                Err(e) => return Some(error(id, e.code, e.kind, e.message)),
            }
        }
        method if method.starts_with("native.") => {
            let params = if request.has_key("params") {
                request["params"].clone()
            } else {
                object! {}
            };
            let outcome=if method=="native.recover" {
                let strict=bytes.len().checked_add(1).is_some_and(|n|n<=MAX_REQUEST_BYTES)&&request.len()==4
                    &&["jsonrpc","id","method","params"].iter().all(|k|request.has_key(k))&&id.dump().len()<=20
                    &&unique_keys(bytes)&&integer_numbers(bytes);
                native.dispatch_recovery_envelope(&params,strict)
            }else{native.dispatch(method, &params)};
            match outcome {
                Ok(result) => result,
                Err(e) => {
                    // Absence is authoritative only for the bound remote status
                    // request; never project it onto another control or operation.
                    if remote && e.kind=="jobAbsent" && !matches!(method,"native.remote.status"|"native.remote.replaceStatus") {
                        return Some(error(id,-32020,"outcomeUnknown","The remote outcome remains unconfirmed."));
                    }
                    return Some(error(
                        id,
                        if e.kind == "invalidParams" {
                            -32602
                        } else if matches!(e.kind.as_str(), "localJobOutcomeUnknown" | "bootstrapOutcomeUnknown") {
                            -32603
                        } else if e.kind == "methodNotFound" {
                            -32601
                        } else {
                            -32020
                        },
                        &e.kind,
                        &e.message,
                    ))
                }
            }
        }
        _ => {
            return Some(error(
                id,
                -32601,
                "methodNotFound",
                "Unknown helper method.",
            ))
        }
    };
    let response = object! {jsonrpc:"2.0",id:id.clone(),result:result};
    if remote {
        let method=request["method"].as_str().unwrap();
        if crate::remote::contract::validate_result(method,&response["result"]).is_err()
            || crate::remote::contract::validate_result_binding(method,&request["params"],&response["result"]).is_err()
            || !crate::remote::contract::result_envelope_ok(method,&response) {
            return Some(error(id,-32020,"responseTooLarge","Remote response could not be represented; reconcile the original request and job identity."));
        }
    }
    if launch && native_value_bytes(&response).is_none_or(|n|n >= MAX_REQUEST_BYTES) {
        return Some(error(id, -32603, "responseTooLarge", "Launch response exceeded its limit; reconcile the original control and durable identities."));
    }
    if catalog {
        let terminal = request["method"] == "native.catalog.status" && !request["params"]["requestKey"].is_null()
            && matches!(response["result"]["phase"].as_str(), Some("succeeded" | "failed" | "cancelled"));
        let limit = if terminal {crate::catalog::contract::TERMINAL_LIMIT} else {crate::catalog::contract::CONTROL_LIMIT};
        if native_value_bytes(&response).is_none_or(|n| n + 1 > limit) {
            return Some(error(id, -32020, "responseTooLarge", "Complete catalog response exceeds its wire limit."));
        }
    }
    if request["method"]=="native.library.inspect"
        && native_value_bytes(&response).is_none_or(|n|n+1>MAX_REQUEST_BYTES) {
        return Some(error(id,-32020,"responseTooLarge","Library inspection exceeds its complete control-frame budget."));
    }
    if recovery_inspect && native_value_bytes(&response).is_none_or(|n|n+1>MAX_REQUEST_BYTES) {
        return Some(error(id,-32020,"responseTooLarge","Recovery inspection exceeds its complete control-frame budget."));
    }
    Some(response)
}

// The shared legacy JSON parser accepts duplicate keys. New launch envelopes
// reject them at every nesting level without altering historical RPC admission.
pub(crate) fn unique_keys(bytes: &[u8]) -> bool {
    fn ws(b:&[u8],i:&mut usize){while b.get(*i).is_some_and(u8::is_ascii_whitespace){*i+=1;}}
    fn string(b:&[u8],i:&mut usize)->Option<String>{
        let start=*i;if b.get(*i)!=Some(&b'"'){return None;}*i+=1;
        while let Some(c)=b.get(*i){*i+=1;if *c==b'\\'{*i+=1;}else if *c==b'"'{return json::parse(std::str::from_utf8(&b[start..*i]).ok()?).ok()?.as_str().map(str::to_owned);}}
        None
    }
    fn value(b:&[u8],i:&mut usize,depth:usize)->Option<()> {
        if depth>16{return None;}ws(b,i);
        match b.get(*i)? {
            b'{' => { *i+=1;ws(b,i);let mut keys=std::collections::BTreeSet::new();
                if b.get(*i)==Some(&b'}'){*i+=1;return Some(());}
                loop {ws(b,i);if !keys.insert(string(b,i)?){return None;}ws(b,i);if b.get(*i)!=Some(&b':'){return None;}*i+=1;value(b,i,depth+1)?;ws(b,i);
                    match b.get(*i)?{b'}'=>{*i+=1;break;},b','=>*i+=1,_=>return None,}}
            },
            b'[' => {*i+=1;ws(b,i);if b.get(*i)==Some(&b']'){*i+=1;return Some(());}loop{value(b,i,depth+1)?;ws(b,i);match b.get(*i)?{b']'=>{*i+=1;break;},b','=>*i+=1,_=>return None,}}},
            b'"'=>{string(b,i)?;},
            _=>{let start=*i;while b.get(*i).is_some_and(|c|!c.is_ascii_whitespace() && !b",]}".contains(c)){*i+=1;}if start==*i{return None;}},
        }Some(())
    }
    let mut i=0;if value(bytes,&mut i,0).is_none(){return false;}ws(bytes,&mut i);i==bytes.len()
}

/// Catalog/transport numbers have canonical integer spelling. The legacy JSON
/// parser otherwise normalizes fractions/exponents before typed admission.
pub(crate) fn integer_numbers(bytes: &[u8]) -> bool {
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            i += 1;
            while i < bytes.len() { let b = bytes[i]; i += 1; if b == b'\\' { i += 1; } else if b == b'"' { break; } }
        } else if bytes[i] == b'-' || bytes[i].is_ascii_digit() {
            let start = i; i += 1;
            while i < bytes.len() && !b",]} \t\r\n".contains(&bytes[i]) { i += 1; }
            let number = &bytes[start..i];
            let digits = if number[0] == b'-' { &number[1..] } else { number };
            if digits.is_empty() || digits.len() > 20 || !digits.iter().all(u8::is_ascii_digit)
                || (digits.len() > 1 && digits[0] == b'0') || number == b"-0" { return false; }
        } else { i += 1; }
    }
    true
}

/// Serialization is buffered under a hard cap so an oversize response can be
/// replaced by an error before any partial JSON reaches the transport.
struct BoundedResponse(Vec<u8>);
impl Write for BoundedResponse {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > (MAX_RESPONSE_BYTES - 1).saturating_sub(self.0.len()) {
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "response limit",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn respond<W: Write>(response: JsonValue, output: &mut W) -> io::Result<()> {
    let mut bytes = BoundedResponse(Vec::new());
    if response.write(&mut bytes).is_err() {
        bytes.0.clear();
        error(
            response["id"].clone(),
            -32603,
            "responseTooLarge",
            "Response exceeds the 32 MiB limit. A dispatched mutation may have completed; retain its operation ID and reconcile before retrying.",
        )
        .write(&mut bytes)?;
    }
    bytes.0.push(b'\n');
    output.write_all(&bytes.0)?;
    output.flush()
}

pub fn serve<R: BufRead, W: Write>(mut input: R, mut output: W) -> io::Result<()> {
    let mut native = crate::native::NativeService::new();
    let mut line = Vec::new();
    let mut oversized = false;
    loop {
        let chunk = input.fill_buf()?;
        let eof = chunk.is_empty();
        let newline = chunk.iter().position(|byte| *byte == b'\n');
        let count = newline.unwrap_or(chunk.len());
        if count > MAX_REQUEST_BYTES.saturating_sub(line.len()) {
            oversized = true;
        }
        if !oversized {
            line.extend_from_slice(&chunk[..count]);
        }
        let consumed = count + usize::from(newline.is_some());
        input.consume(consumed);
        if newline.is_some() || (eof && (oversized || !line.is_empty())) {
            let response = if oversized {
                Some(error(
                    JsonValue::Null,
                    -32600,
                    "invalidRequest",
                    "Request line exceeds the 64 KiB limit.",
                ))
            } else {
                dispatch(&mut native, &line)
            };
            if let Some(response) = response {
                respond(response, &mut output)?;
            }
            // Explicit launch close retires the whole main helper. Merely dropping
            // a thread's Child cannot reparent a persistent Steam process.
            if native.retiring() { return Ok(()); }
            line.clear();
            oversized = false;
        }
        if eof {
            return Ok(());
        }
    }
}
