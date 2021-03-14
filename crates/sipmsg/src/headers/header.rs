use crate::{
    common::{bnfcore::*, errorparse::SipParseError, nom_wrappers::from_utf8_nom, take_sws_token},
    headers::{
        parsers::ExtensionParser,
        traits::{HeaderValueParserFn, SipHeaderParser},
        GenericParams, SipRFCHeader, SipUri,
    },
};
use alloc::collections::{BTreeMap, VecDeque};
use core::str;
use nom::{bytes::complete::take_while1, character::complete};
use unicase::Ascii;

// All possible types of value
// Glossary: R-required, O-optional
#[derive(PartialEq, Debug)]
pub enum HeaderValueType {
    EmptyValue,           // SIP header with empty value. Haven't tags
    TokenValue,           // Haven't tags. Simple value of token chars
    Digit,                // Haven't tags, just *[0-9] in HeaderValue.vstr
    AbsoluteURI,          // tags: AbsoluteURI(R),
    QuotedValue,          // tags: PureValue(R)
    AuthentificationInfo, // tags: AinfoType(R), AinfoValue(R)
    CSeq,                 // tags: Number(R), Method(R)
    DateString,           // Haven't tags
    Utf8Text,             // Haven't tags
    Version,              // tags: Major(R) Minor(O)

    // Authorization     =  "Authorization" HCOLON credentials
    // credentials       =  ("Digest" LWS digest-response)
    // other-response
    AuthorizationDigest, // tags: username / realm / nonce / digest-uri
    //       / dresponse / algorithm / cnonce
    //       / opaque / QopValue / nonce-count / auth-param

    // callid   =  word [ "@" word ]
    CallID, // tags: ID(R), Host(O)

    // Call-Info   =  "Call-Info" HCOLON info *(COMMA info)
    CallInfo, // tags: PureValue(R)

    /// Contact, From, To, Record-Route, Route headers
    NameAddr, // tags: Star(O), DisplayName(O), AbsoluteURI(O)

    Timestamp, // tags: TimeVal, Delay

    RetryAfter, // tags: Seconds(R), Comment(O)
    UserAgent,  // haven't tags,

    Via,     // tags: ProtocolName(R),ProtocolVersion(R),ProtocolTransport(R), Host(R), Port(O)
    Warning, // tags: WarnCode(R), WarnAgent(R), WarnText(R)
    ExtensionHeader, // No tags
}

#[derive(PartialEq, Debug, Eq, PartialOrd, Ord)]
pub enum HeaderTagType {
    PureValue,
    AinfoType,   // nextnonce, qop, rspauth, etc.
    AinfoValue,  // value after equal without quotes
    AbsoluteURI, // absolute uri without qoutes
    // Auth params: (Headers: Authorization, Proxy-Authenticate)
    AuthSchema,
    Username,
    Domain,
    Realm,
    Nonce,
    DigestUri, // digest-uri-value  =  Request-URI ; as defined in Section 25
    Dresponse,
    Algorithm,
    Cnonce,
    Opaque,
    Stale,
    QopValue,
    NonceCount,
    ///////////////
    Number,
    Method,
    ID,
    Host,
    Port,
    Star, // alway must be equal to *
    DisplayName,
    Seconds,
    Comment,
    Major,
    Minor,
    TimveVal,
    Delay,

    ProtocolName,
    ProtocolVersion,
    ProtocolTransport,

    WarnCode,
    WarnAgent,
    WarnText,
}

pub type HeaderTags<'a> = BTreeMap<HeaderTagType, &'a [u8]>;

#[derive(PartialEq, Debug)]
pub struct HeaderValue<'a> {
    pub vstr: &'a str,
    pub vtype: HeaderValueType,
    vtags: Option<HeaderTags<'a>>,
    sip_uri: Option<SipUri<'a>>,
}

impl<'a> HeaderValue<'a> {
    pub fn create_empty_value() -> HeaderValue<'a> {
        HeaderValue {
            vstr: "",
            vtype: HeaderValueType::EmptyValue,
            vtags: None,
            sip_uri: None,
        }
    }

    pub fn new(
        val: &'a [u8],
        vtype: HeaderValueType,
        vtags: Option<HeaderTags<'a>>,
        sip_uri: Option<SipUri<'a>>,
    ) -> nom::IResult<&'a [u8], HeaderValue<'a>, SipParseError<'a>> {
        let (_, vstr) = from_utf8_nom(val)?;

        Ok((
            val,
            HeaderValue {
                vstr: vstr,
                vtype: vtype,
                vtags: vtags,
                sip_uri: sip_uri,
            },
        ))
    }

    pub fn tags(&self) -> Option<&HeaderTags<'a>> {
        self.vtags.as_ref()
    }

    pub fn sip_uri(&self) -> Option<&SipUri<'a>> {
        self.sip_uri.as_ref()
    }
}

#[derive(PartialEq, Debug)]
/// [rfc3261 section-7.3](https://tools.ietf.org/html/rfc3261#section-7.3)
pub struct Header<'a> {
    /// SIP header name
    pub name: Ascii<&'a str>,
    /// SIP header value
    pub value: HeaderValue<'a>,
    /// SIP parameters
    parameters: Option<GenericParams<'a>>,
    /// Raw representation part of string that contain value and params
    pub raw_value_param: &'a[u8]
}

impl<'a> Header<'a> {
    pub fn new(
        name: &'a str,
        value: HeaderValue<'a>,
        parameters: Option<GenericParams<'a>>,
        raw_value_param: &'a[u8],
    ) -> Header<'a> {
        Header {
            name: { Ascii::new(name) },
            value: value,
            parameters: parameters,
            raw_value_param: raw_value_param
        }
    }

    pub fn params(&self) -> Option<&GenericParams<'a>> {
        self.parameters.as_ref()
    }

    pub fn find_parser(header_name: &'a str) -> (Option<SipRFCHeader>, HeaderValueParserFn) {
        match SipRFCHeader::from_str(&header_name) {
            Some(rfc_header) => (Some(rfc_header), rfc_header.get_parser()),
            None => (None, ExtensionParser::take_value),
        }
    }

    pub fn take_name(source_input: &'a [u8]) -> nom::IResult<&[u8], &'a str, SipParseError> {
        let (input, header_name) = take_while1(is_token_char)(source_input)?;
        let (input, _) = take_sws_token::colon(input)?;
        match str::from_utf8(header_name) {
            Ok(hdr_str) => Ok((input, hdr_str)),
            Err(_) => sip_parse_error!(1, "Bad header name"),
        }
    }

    /// Should return COMMA, SEMI or '\r\n' in first argument
    pub fn take_value(
        input: &'a [u8],
        parser: HeaderValueParserFn,
    ) -> nom::IResult<&'a [u8], (HeaderValue<'a>, Option<GenericParams<'a>>), SipParseError<'a>>
    {
        if is_crlf(input) {
            return Ok((input, (HeaderValue::create_empty_value(), None))); // This is header with empty value
        }

        let (inp, value) = parser(input)?;
        // let (_, value) = from_utf8_nom(value)?;

        // skip whitespaces after take value
        let (inp, _) = complete::space0(inp)?;
        if inp.is_empty() {
            return sip_parse_error!(1, "Error parse header value");
        }
        if inp[0] != b',' && inp[0] != b';' && inp[0] != b' ' && !is_crlf(inp) {
            return sip_parse_error!(2, "Error parse header value");
        }

        if inp[0] == b';' {
            let (inp, params) = Header::try_take_parameters(inp)?;
            return Ok((inp, (value, params)));
        }
        Ok((inp, (value, None)))
    }

    fn try_take_parameters(
        input: &'a [u8],
    ) -> nom::IResult<&'a [u8], Option<GenericParams<'a>>, SipParseError<'a>> {
        if input.is_empty() || input[0] != b';' {
            return Ok((input, None));
        }
        let (input, parameters) = GenericParams::parse(input)?;
        Ok((input, Some(parameters)))
    }

    pub fn parse(
        input: &'a [u8],
    ) -> nom::IResult<&[u8], (Option<SipRFCHeader>, VecDeque<Header<'a>>), SipParseError> {
        let mut headers = VecDeque::new();
        let (input, header_name) = Header::take_name(input)?;
        let (rfc_type, value_parser) = Header::find_parser(header_name);
        let mut inp = input;
        loop {
            let (input, (value, params)) = Header::take_value(inp, value_parser)?;
            headers.push_back(Header::new(header_name, value, params, &inp[..inp.len() - input.len()]));
            if input.is_empty() {
                return sip_parse_error!(1, "header input is empty");
            }
            if input[0] == b',' {
                let (input, _) = take_sws_token::comma(input)?;
                inp = input;
                continue;
            }
            inp = input;
            break;
        }
        Ok((inp, (rfc_type, headers)))
    }
}
