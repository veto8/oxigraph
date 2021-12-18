//! Utilities to read RDF graphs and datasets.

use crate::io::{DatasetFormat, GraphFormat};
use crate::model::*;
use oxiri::{Iri, IriParseError};
use rio_api::model as rio;
use rio_api::parser::{QuadsParser, TriplesParser};
use rio_turtle::{NQuadsParser, NTriplesParser, TriGParser, TurtleError, TurtleParser};
use rio_xml::{RdfXmlError, RdfXmlParser};
use std::collections::HashMap;
use std::error::Error;
use std::io::BufRead;
use std::{fmt, io};

/// Parsers for RDF graph serialization formats.
///
/// It currently supports the following formats:
/// * [N-Triples](https://www.w3.org/TR/n-triples/) ([`GraphFormat::NTriples`](super::GraphFormat::NTriples))
/// * [Turtle](https://www.w3.org/TR/turtle/) ([`GraphFormat::Turtle`](super::GraphFormat::Turtle))
/// * [RDF/XML](https://www.w3.org/TR/rdf-syntax-grammar/) ([`GraphFormat::RdfXml`](super::GraphFormat::RdfXml))
///
/// ```
/// use oxigraph::io::{GraphFormat, GraphParser};
/// use std::io::Cursor;
///
/// let file = "<http://example.com/s> <http://example.com/p> <http://example.com/o> .";
///
/// let parser = GraphParser::from_format(GraphFormat::NTriples);
/// let triples = parser.read_triples(Cursor::new(file))?.collect::<Result<Vec<_>,_>>()?;
///
///assert_eq!(triples.len(), 1);
///assert_eq!(triples[0].subject.to_string(), "<http://example.com/s>");
/// # std::io::Result::Ok(())
/// ```
pub struct GraphParser {
    format: GraphFormat,
    base_iri: Option<Iri<String>>,
}

impl GraphParser {
    /// Builds a parser for the given format.
    pub fn from_format(format: GraphFormat) -> Self {
        Self {
            format,
            base_iri: None,
        }
    }

    /// Provides an IRI that could be used to resolve the file relative IRIs.
    ///
    /// ```
    /// use oxigraph::io::{GraphFormat, GraphParser};
    /// use std::io::Cursor;
    ///
    /// let file = "</s> </p> </o> .";
    ///
    /// let parser = GraphParser::from_format(GraphFormat::Turtle).with_base_iri("http://example.com")?;
    /// let triples = parser.read_triples(Cursor::new(file))?.collect::<Result<Vec<_>,_>>()?;
    ///
    ///assert_eq!(triples.len(), 1);
    ///assert_eq!(triples[0].subject.to_string(), "<http://example.com/s>");
    /// # Result::<_,Box<dyn std::error::Error>>::Ok(())
    /// ```
    pub fn with_base_iri(mut self, base_iri: impl Into<String>) -> Result<Self, IriParseError> {
        self.base_iri = Some(Iri::parse(base_iri.into())?);
        Ok(self)
    }

    /// Executes the parsing itself on a [`BufRead`](std::io::BufRead) implementation and returns an iterator of triples.
    #[allow(clippy::unnecessary_wraps)]
    pub fn read_triples<R: BufRead>(&self, reader: R) -> Result<TripleReader<R>, ParserError> {
        Ok(TripleReader {
            mapper: RioMapper::default(),
            parser: match self.format {
                GraphFormat::NTriples => TripleReaderKind::NTriples(NTriplesParser::new(reader)),
                GraphFormat::Turtle => {
                    TripleReaderKind::Turtle(TurtleParser::new(reader, self.base_iri.clone()))
                }
                GraphFormat::RdfXml => {
                    TripleReaderKind::RdfXml(RdfXmlParser::new(reader, self.base_iri.clone()))
                }
            },
            buffer: Vec::new(),
        })
    }
}

/// An iterator yielding read triples.
/// Could be built using a [`GraphParser`].
///
/// ```
/// use oxigraph::io::{GraphFormat, GraphParser};
/// use std::io::Cursor;
///
/// let file = "<http://example.com/s> <http://example.com/p> <http://example.com/o> .";
///
/// let parser = GraphParser::from_format(GraphFormat::NTriples);
/// let triples = parser.read_triples(Cursor::new(file))?.collect::<Result<Vec<_>,_>>()?;
///
///assert_eq!(triples.len(), 1);
///assert_eq!(triples[0].subject.to_string(), "<http://example.com/s>");
/// # std::io::Result::Ok(())
/// ```
#[must_use]
pub struct TripleReader<R: BufRead> {
    mapper: RioMapper,
    parser: TripleReaderKind<R>,
    buffer: Vec<Triple>,
}

enum TripleReaderKind<R: BufRead> {
    NTriples(NTriplesParser<R>),
    Turtle(TurtleParser<R>),
    RdfXml(RdfXmlParser<R>),
}

impl<R: BufRead> Iterator for TripleReader<R> {
    type Item = Result<Triple, ParserError>;

    fn next(&mut self) -> Option<Result<Triple, ParserError>> {
        loop {
            if let Some(r) = self.buffer.pop() {
                return Some(Ok(r));
            }

            if let Err(error) = match &mut self.parser {
                TripleReaderKind::NTriples(parser) => {
                    Self::read(parser, &mut self.buffer, &mut self.mapper)
                }
                TripleReaderKind::Turtle(parser) => {
                    Self::read(parser, &mut self.buffer, &mut self.mapper)
                }
                TripleReaderKind::RdfXml(parser) => {
                    Self::read(parser, &mut self.buffer, &mut self.mapper)
                }
            }? {
                return Some(Err(error));
            }
        }
    }
}

impl<R: BufRead> TripleReader<R> {
    fn read<P: TriplesParser>(
        parser: &mut P,
        buffer: &mut Vec<Triple>,
        mapper: &mut RioMapper,
    ) -> Option<Result<(), ParserError>>
    where
        ParserError: From<P::Error>,
    {
        if parser.is_end() {
            None
        } else if let Err(e) = parser.parse_step(&mut |t| {
            buffer.push(mapper.triple(&t));
            Ok(())
        }) {
            Some(Err(e))
        } else {
            Some(Ok(()))
        }
    }
}

/// A parser for RDF dataset serialization formats.
///
/// It currently supports the following formats:
/// * [N-Quads](https://www.w3.org/TR/n-quads/) ([`DatasetFormat::NQuads`](super::DatasetFormat::NQuads))
/// * [TriG](https://www.w3.org/TR/trig/) ([`DatasetFormat::TriG`](super::DatasetFormat::TriG))
///
/// ```
/// use oxigraph::io::{DatasetFormat, DatasetParser};
/// use std::io::Cursor;
///
/// let file = "<http://example.com/s> <http://example.com/p> <http://example.com/o> <http://example.com/g> .";
///
/// let parser = DatasetParser::from_format(DatasetFormat::NQuads);
/// let quads = parser.read_quads(Cursor::new(file))?.collect::<Result<Vec<_>,_>>()?;
///
///assert_eq!(quads.len(), 1);
///assert_eq!(quads[0].subject.to_string(), "<http://example.com/s>");
/// # std::io::Result::Ok(())
/// ```
pub struct DatasetParser {
    format: DatasetFormat,
    base_iri: Option<Iri<String>>,
}

impl DatasetParser {
    /// Builds a parser for the given format.
    pub fn from_format(format: DatasetFormat) -> Self {
        Self {
            format,
            base_iri: None,
        }
    }

    /// Provides an IRI that could be used to resolve the file relative IRIs.
    ///
    /// ```
    /// use oxigraph::io::{DatasetFormat, DatasetParser};
    /// use std::io::Cursor;
    ///
    /// let file = "<g> { </s> </p> </o> }";
    ///
    /// let parser = DatasetParser::from_format(DatasetFormat::TriG).with_base_iri("http://example.com")?;
    /// let triples = parser.read_quads(Cursor::new(file))?.collect::<Result<Vec<_>,_>>()?;
    ///
    ///assert_eq!(triples.len(), 1);
    ///assert_eq!(triples[0].subject.to_string(), "<http://example.com/s>");
    /// # Result::<_,Box<dyn std::error::Error>>::Ok(())
    /// ```
    pub fn with_base_iri(mut self, base_iri: impl Into<String>) -> Result<Self, IriParseError> {
        self.base_iri = Some(Iri::parse(base_iri.into())?);
        Ok(self)
    }

    /// Executes the parsing itself on a [`BufRead`](std::io::BufRead) implementation and returns an iterator of quads.
    #[allow(clippy::unnecessary_wraps)]
    pub fn read_quads<R: BufRead>(&self, reader: R) -> Result<QuadReader<R>, ParserError> {
        Ok(QuadReader {
            mapper: RioMapper::default(),
            parser: match self.format {
                DatasetFormat::NQuads => QuadReaderKind::NQuads(NQuadsParser::new(reader)),
                DatasetFormat::TriG => {
                    QuadReaderKind::TriG(TriGParser::new(reader, self.base_iri.clone()))
                }
            },
            buffer: Vec::new(),
        })
    }
}

/// An iterator yielding read quads.
/// Could be built using a [`DatasetParser`].
///
/// ```
/// use oxigraph::io::{DatasetFormat, DatasetParser};
/// use std::io::Cursor;
///
/// let file = "<http://example.com/s> <http://example.com/p> <http://example.com/o> <http://example.com/g> .";
///
/// let parser = DatasetParser::from_format(DatasetFormat::NQuads);
/// let quads = parser.read_quads(Cursor::new(file))?.collect::<Result<Vec<_>,_>>()?;
///
///assert_eq!(quads.len(), 1);
///assert_eq!(quads[0].subject.to_string(), "<http://example.com/s>");
/// # std::io::Result::Ok(())
/// ```
#[must_use]
pub struct QuadReader<R: BufRead> {
    mapper: RioMapper,
    parser: QuadReaderKind<R>,
    buffer: Vec<Quad>,
}

enum QuadReaderKind<R: BufRead> {
    NQuads(NQuadsParser<R>),
    TriG(TriGParser<R>),
}

impl<R: BufRead> Iterator for QuadReader<R> {
    type Item = Result<Quad, ParserError>;

    fn next(&mut self) -> Option<Result<Quad, ParserError>> {
        loop {
            if let Some(r) = self.buffer.pop() {
                return Some(Ok(r));
            }

            if let Err(error) = match &mut self.parser {
                QuadReaderKind::NQuads(parser) => {
                    Self::read(parser, &mut self.buffer, &mut self.mapper)
                }
                QuadReaderKind::TriG(parser) => {
                    Self::read(parser, &mut self.buffer, &mut self.mapper)
                }
            }? {
                return Some(Err(error));
            }
        }
    }
}

impl<R: BufRead> QuadReader<R> {
    fn read<P: QuadsParser>(
        parser: &mut P,
        buffer: &mut Vec<Quad>,
        mapper: &mut RioMapper,
    ) -> Option<Result<(), ParserError>>
    where
        ParserError: From<P::Error>,
    {
        if parser.is_end() {
            None
        } else if let Err(e) = parser.parse_step(&mut |t| {
            buffer.push(mapper.quad(&t));
            Ok(())
        }) {
            Some(Err(e))
        } else {
            Some(Ok(()))
        }
    }
}

#[derive(Default)]
struct RioMapper {
    bnode_map: HashMap<String, BlankNode>,
}

impl<'a> RioMapper {
    fn named_node(node: rio::NamedNode<'a>) -> NamedNode {
        NamedNode::new_unchecked(node.iri)
    }

    fn blank_node(&mut self, node: rio::BlankNode<'a>) -> BlankNode {
        self.bnode_map
            .entry(node.id.to_owned())
            .or_insert_with(BlankNode::default)
            .clone()
    }

    fn literal(literal: rio::Literal<'a>) -> Literal {
        match literal {
            rio::Literal::Simple { value } => Literal::new_simple_literal(value),
            rio::Literal::LanguageTaggedString { value, language } => {
                Literal::new_language_tagged_literal_unchecked(value, language)
            }
            rio::Literal::Typed { value, datatype } => {
                Literal::new_typed_literal(value, Self::named_node(datatype))
            }
        }
    }

    fn subject(&mut self, node: rio::Subject<'a>) -> Subject {
        match node {
            rio::Subject::NamedNode(node) => Self::named_node(node).into(),
            rio::Subject::BlankNode(node) => self.blank_node(node).into(),
            rio::Subject::Triple(triple) => self.triple(triple).into(),
        }
    }

    fn term(&mut self, node: rio::Term<'a>) -> Term {
        match node {
            rio::Term::NamedNode(node) => Self::named_node(node).into(),
            rio::Term::BlankNode(node) => self.blank_node(node).into(),
            rio::Term::Literal(literal) => Self::literal(literal).into(),
            rio::Term::Triple(triple) => self.triple(triple).into(),
        }
    }

    fn triple(&mut self, triple: &rio::Triple<'a>) -> Triple {
        Triple {
            subject: self.subject(triple.subject),
            predicate: Self::named_node(triple.predicate),
            object: self.term(triple.object),
        }
    }

    fn graph_name(&mut self, graph_name: Option<rio::GraphName<'a>>) -> GraphName {
        match graph_name {
            Some(rio::GraphName::NamedNode(node)) => Self::named_node(node).into(),
            Some(rio::GraphName::BlankNode(node)) => self.blank_node(node).into(),
            None => GraphName::DefaultGraph,
        }
    }

    fn quad(&mut self, quad: &rio::Quad<'a>) -> Quad {
        Quad {
            subject: self.subject(quad.subject),
            predicate: Self::named_node(quad.predicate),
            object: self.term(quad.object),
            graph_name: self.graph_name(quad.graph_name),
        }
    }
}

/// Error returned during RDF format parsing.
#[derive(Debug)]
pub enum ParserError {
    /// I/O error during parsing (file not found...).
    Io(io::Error),
    /// An error in the file syntax.
    Syntax(SyntaxError),
}

impl ParserError {
    pub(crate) fn invalid_base_iri(iri: &str, error: IriParseError) -> Self {
        Self::Syntax(SyntaxError {
            inner: SyntaxErrorKind::BaseIri {
                iri: iri.to_owned(),
                error,
            },
        })
    }
}

impl fmt::Display for ParserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => e.fmt(f),
            Self::Syntax(e) => e.fmt(f),
        }
    }
}

impl Error for ParserError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Syntax(e) => Some(e),
        }
    }
}

#[allow(clippy::fallible_impl_from)]
impl From<TurtleError> for ParserError {
    fn from(error: TurtleError) -> Self {
        let error = io::Error::from(error);
        if error.get_ref().map_or(false, |e| e.is::<TurtleError>()) {
            Self::Syntax(SyntaxError {
                inner: SyntaxErrorKind::Turtle(*error.into_inner().unwrap().downcast().unwrap()),
            })
        } else {
            Self::Io(error)
        }
    }
}

#[allow(clippy::fallible_impl_from)]
impl From<RdfXmlError> for ParserError {
    fn from(error: RdfXmlError) -> Self {
        let error = io::Error::from(error);
        if error.get_ref().map_or(false, |e| e.is::<RdfXmlError>()) {
            Self::Syntax(SyntaxError {
                inner: SyntaxErrorKind::RdfXml(*error.into_inner().unwrap().downcast().unwrap()),
            })
        } else {
            Self::Io(error)
        }
    }
}

impl From<TermParseError> for ParserError {
    fn from(error: TermParseError) -> Self {
        Self::Syntax(SyntaxError {
            inner: SyntaxErrorKind::Term(error),
        })
    }
}

impl From<io::Error> for ParserError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<SyntaxError> for ParserError {
    fn from(error: SyntaxError) -> Self {
        Self::Syntax(error)
    }
}

impl From<ParserError> for io::Error {
    fn from(error: ParserError) -> Self {
        match error {
            ParserError::Io(error) => error,
            ParserError::Syntax(error) => error.into(),
        }
    }
}

impl From<quick_xml::Error> for ParserError {
    fn from(error: quick_xml::Error) -> Self {
        match error {
            quick_xml::Error::Io(error) => Self::Io(error),
            error => Self::Syntax(SyntaxError {
                inner: SyntaxErrorKind::Xml(error),
            }),
        }
    }
}

/// An error in the syntax of the parsed file
#[derive(Debug)]
pub struct SyntaxError {
    pub(crate) inner: SyntaxErrorKind,
}

#[derive(Debug)]
pub(crate) enum SyntaxErrorKind {
    Turtle(TurtleError),
    RdfXml(RdfXmlError),
    BaseIri { iri: String, error: IriParseError },
    Xml(quick_xml::Error),
    Term(TermParseError),
    Msg { msg: String },
}

impl SyntaxError {
    /// Builds an error from a printable error message.
    pub(crate) fn msg(msg: impl Into<String>) -> Self {
        Self {
            inner: SyntaxErrorKind::Msg { msg: msg.into() },
        }
    }
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.inner {
            SyntaxErrorKind::Turtle(e) => e.fmt(f),
            SyntaxErrorKind::RdfXml(e) => e.fmt(f),
            SyntaxErrorKind::BaseIri { iri, error } => {
                write!(f, "Invalid base IRI '{}': {}", iri, error)
            }
            SyntaxErrorKind::Xml(e) => e.fmt(f),
            SyntaxErrorKind::Term(e) => e.fmt(f),
            SyntaxErrorKind::Msg { msg } => f.write_str(msg),
        }
    }
}

impl Error for SyntaxError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.inner {
            SyntaxErrorKind::Turtle(e) => Some(e),
            SyntaxErrorKind::RdfXml(e) => Some(e),
            SyntaxErrorKind::Xml(e) => Some(e),
            SyntaxErrorKind::Term(e) => Some(e),
            SyntaxErrorKind::BaseIri { .. } | SyntaxErrorKind::Msg { .. } => None,
        }
    }
}

impl From<SyntaxError> for io::Error {
    fn from(error: SyntaxError) -> Self {
        match error.inner {
            SyntaxErrorKind::Turtle(error) => error.into(),
            SyntaxErrorKind::RdfXml(error) => error.into(),
            SyntaxErrorKind::BaseIri { iri, error } => Self::new(
                io::ErrorKind::InvalidInput,
                format!("Invalid IRI '{}': {}", iri, error),
            ),
            SyntaxErrorKind::Xml(error) => match error {
                quick_xml::Error::Io(error) => error,
                quick_xml::Error::UnexpectedEof(error) => {
                    Self::new(io::ErrorKind::UnexpectedEof, error)
                }
                error => Self::new(io::ErrorKind::InvalidData, error),
            },
            SyntaxErrorKind::Term(error) => Self::new(io::ErrorKind::InvalidData, error),
            SyntaxErrorKind::Msg { msg } => Self::new(io::ErrorKind::InvalidData, msg),
        }
    }
}
