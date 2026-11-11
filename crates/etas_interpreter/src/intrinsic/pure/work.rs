use etas_builtin::{
    BuiltinError,
    text::case::{CaseMapping, TextCaseWork},
    text::format::{TextFormatProgress, TextFormatWork},
    text::output::TextOutputWork,
    text::query::{TextQuery, TextQueryWork},
    text::ranges::{TextTrimKind, TextTrimWork},
    text::reverse::{TextReverseKind, TextReverseWork},
    text::search::{TextSearchKind, TextSearchWork},
    text::unicode::{NormalizationForm, TextCasefoldWork, TextNormalizeWork},
    work::{WorkControl, WorkPoll},
};

use crate::{
    control::Frame,
    eval::ForLoopResume,
    intrinsic::dispatch::CheckedPureIntrinsicCall,
    value::{
        ArrayValue, InterpValue, OwnedTextCursor, StringValue, TextRangeProjector,
        iteration::{IterationPoll, IterationSource},
    },
};
use etas_types::TypeId;

use super::{
    PureAbiProjector,
    abi::{AdapterError, borrowed, output::from_builtin_for_type},
};

const WORK_QUANTUM: usize = 64 * 1024;
const MAX_NATIVE_OUTPUT_BYTES: usize = isize::MAX as usize;
const MAX_NATIVE_SCRATCH_BYTES: usize = isize::MAX as usize;

#[derive(Clone)]
pub(crate) struct PendingNativeWork {
    call: NativeWorkOwner,
    state: TextWorkState,
}

#[derive(Clone)]
enum NativeWorkOwner {
    Intrinsic(CheckedPureIntrinsicCall),
    ForLoop(Box<NativeForLoop>),
}

impl std::ops::Deref for NativeWorkOwner {
    type Target = CheckedPureIntrinsicCall;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Intrinsic(call) => call,
            Self::ForLoop(_) => unreachable!("for-loop native work has no intrinsic descriptor"),
        }
    }
}

#[derive(Clone)]
struct NativeForLoop {
    resume: ForLoopResume,
    frame: Frame,
}

impl std::fmt::Debug for PendingNativeWork {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PendingNativeWork")
            .field("intrinsic", &self.call().map(|call| call.intrinsic))
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
enum TextWorkState {
    Query {
        source: StringValue,
        cursor: OwnedTextCursor,
        query: TextQueryWork,
    },
    IntegerParse {
        source: StringValue,
        cursor: OwnedTextCursor,
        kind: etas_builtin::text::parse::TextIntegerKind,
        work: etas_builtin::text::parse::TextIntegerParseWork,
    },
    FloatParse {
        source: StringValue,
        cursor: OwnedTextCursor,
        kind: etas_builtin::text::parse::TextFloatKind,
        work: etas_builtin::text::parse::TextFloatParseWork,
    },
    Search {
        source: StringValue,
        source_cursor: OwnedTextCursor,
        pattern: StringValue,
        pattern_cursor: OwnedTextCursor,
        pattern_ready: bool,
        search: TextSearchWork,
        finish: TextSearchFinish,
        selected: Option<std::ops::Range<usize>>,
        projection: OwnedTextCursor,
        output: TextOutputWork,
    },
    Repeat {
        source: StringValue,
        cursor: OwnedTextCursor,
        remaining: usize,
        total_bytes: usize,
        output: TextOutputWork,
    },
    Join {
        parts: ArrayValue,
        item_type: TypeId,
        separator: StringValue,
        phase: JoinPhase,
        output: TextOutputWork,
    },
    Case {
        source: StringValue,
        cursor: OwnedTextCursor,
        work: TextCaseWork,
    },
    Trim {
        source: StringValue,
        cursor: OwnedTextCursor,
        work: TextTrimWork,
    },
    Casefold {
        source: StringValue,
        cursor: OwnedTextCursor,
        work: TextCasefoldWork,
    },
    Normalize {
        source: StringValue,
        cursor: OwnedTextCursor,
        work: TextNormalizeWork,
        query: bool,
        comparison: Option<(String, usize)>,
    },
    Reverse {
        source: StringValue,
        cursor: OwnedTextCursor,
        work: TextReverseWork,
    },
    Pad {
        source: StringValue,
        cursor: OwnedTextCursor,
        width: usize,
        fill: char,
        start: bool,
        phase: PadPhase,
        output: TextOutputWork,
    },
    Format {
        template: StringValue,
        template_cursor: OwnedTextCursor,
        arguments: ArrayValue,
        item_type: TypeId,
        argument: Option<(StringValue, OwnedTextCursor)>,
        work: TextFormatWork,
    },
    Collect {
        source: Box<IterationSource>,
        separator: StringValue,
        index: usize,
        values: Vec<InterpValue>,
        kind: TextCollectKind,
        reduction: Option<TextCollectionReduction>,
    },
    Replace {
        source: Box<IterationSource>,
        pattern: StringValue,
        replacement: StringValue,
        limit: Option<usize>,
        index: usize,
        output: Option<StringValue>,
    },
    ReplaceRange {
        source: StringValue,
        source_cursor: OwnedTextCursor,
        range: std::ops::Range<usize>,
        replacement: StringValue,
        replacement_cursor: OwnedTextCursor,
        phase: ReplaceRangePhase,
        total_bytes: usize,
        output: TextOutputWork,
    },
    Grapheme {
        source: StringValue,
        scan: OwnedTextCursor,
        work: etas_builtin::text::iteration::TextSequenceWork,
        ordinal: usize,
        requested: Option<std::ops::Range<usize>>,
        selected_start: Option<usize>,
        selected_end: Option<usize>,
        projection: OwnedTextCursor,
        output: TextOutputWork,
    },
    Scalar {
        source: StringValue,
        scan: OwnedTextCursor,
        work: etas_builtin::text::iteration::TextSequenceWork,
        ordinal: usize,
        operation: TextScalarOperation,
        selected_start: Option<usize>,
        selected_end: Option<usize>,
        projection: OwnedTextCursor,
        output: TextOutputWork,
    },
    ByteRange {
        source: StringValue,
        scan: OwnedTextCursor,
        work: etas_builtin::text::iteration::TextSequenceWork,
        operation: TextByteOperation,
        projection: OwnedTextCursor,
        output: TextOutputWork,
    },
    AsciiEq {
        left: StringValue,
        left_cursor: OwnedTextCursor,
        right: StringValue,
        right_cursor: OwnedTextCursor,
    },
    Iteration {
        source: Box<IterationSource>,
        index: usize,
    },
}

#[derive(Clone, Copy)]
enum TextCollectKind {
    Split,
    SplitN(usize),
    SplitOnce,
    RsplitOnce,
}

#[derive(Clone, Copy)]
enum TextSearchFinish {
    Query,
    StripPrefix,
    StripSuffix,
}

#[derive(Clone, Copy)]
enum ReplaceRangePhase {
    Prefix,
    Skip,
    Replacement,
    Suffix,
}

#[derive(Clone)]
enum TextScalarOperation {
    Get {
        target: Option<usize>,
    },
    Slice {
        requested: Option<std::ops::Range<usize>>,
        optional: bool,
    },
}

#[derive(Clone)]
enum TextByteOperation {
    Slice {
        range: Option<std::ops::Range<usize>>,
        optional: bool,
        start_valid: bool,
        end_valid: bool,
    },
    IsBoundary {
        target: Option<usize>,
    },
}

#[derive(Clone)]
struct TextCollectionReduction {
    values: Vec<InterpValue>,
    index: usize,
    output: Option<StringValue>,
    finish: TextCollectionFinish,
}

#[derive(Clone)]
enum TextCollectionFinish {
    SplitN(Vec<InterpValue>),
    SplitOnce(InterpValue),
    RsplitOnce(InterpValue),
}

#[derive(Clone)]
enum JoinPhase {
    Size {
        index: usize,
        bytes: usize,
    },
    Write {
        index: usize,
        separator_pending: bool,
        current: Option<(StringValue, OwnedTextCursor)>,
    },
}

#[derive(Clone)]
enum PadPhase {
    Count { scalars: usize },
    Padding { remaining: usize, source_next: bool },
    Source { padding_after: usize },
}

pub(crate) enum NativeWorkPoll {
    Pending,
    Ready(InterpValue),
    IterationReady(Option<InterpValue>),
}

impl PendingNativeWork {
    pub(crate) fn start_iteration(
        resume: ForLoopResume,
        source: IterationSource,
        frame: Frame,
    ) -> Self {
        let index = resume.next_index;
        Self {
            call: NativeWorkOwner::ForLoop(Box::new(NativeForLoop { resume, frame })),
            state: TextWorkState::Iteration {
                source: Box::new(source),
                index,
            },
        }
    }

    pub(crate) fn start(
        call: &CheckedPureIntrinsicCall,
        args: &[InterpValue],
        projector: &PureAbiProjector,
    ) -> Result<Option<Self>, AdapterError> {
        if let Some(query) = TextQuery::for_intrinsic(call.intrinsic)
            && matches!(query, TextQuery::CharCount | TextQuery::IsAscii)
        {
            if args.len() != query.arity() {
                return Err(AdapterError::Arity {
                    expected: query.arity(),
                    actual: args.len(),
                });
            }
            let source =
                borrowed::shared_string_for_type(&args[0], call.parameter_types[0], projector)?
                    .clone();
            let query = TextQueryWork::new(query, None).expect("resumable text query");
            return Ok(Some(Self {
                call: NativeWorkOwner::Intrinsic(call.clone()),
                state: TextWorkState::Query {
                    cursor: source.owned_cursor(),
                    source,
                    query,
                },
            }));
        }
        if let Some(work) = Self::start_repeat(call, args, projector)? {
            return Ok(Some(work));
        }
        if let Some(work) = Self::start_join(call, args, projector)? {
            return Ok(Some(work));
        }
        if let Some(work) = Self::start_case(call, args, projector)? {
            return Ok(Some(work));
        }
        if let Some(work) = Self::start_trim(call, args, projector)? {
            return Ok(Some(work));
        }
        if let Some(work) = Self::start_casefold(call, args, projector)? {
            return Ok(Some(work));
        }
        if let Some(work) = Self::start_normalize(call, args, projector)? {
            return Ok(Some(work));
        }
        if let Some(work) = Self::start_reverse(call, args, projector)? {
            return Ok(Some(work));
        }
        if let Some(work) = Self::start_pad(call, args, projector)? {
            return Ok(Some(work));
        }
        if let Some(work) = Self::start_format(call, args, projector)? {
            return Ok(Some(work));
        }
        if let Some(work) = Self::start_collect(call, args, projector)? {
            return Ok(Some(work));
        }
        if let Some(work) = Self::start_replace(call, args, projector)? {
            return Ok(Some(work));
        }
        if let Some(work) = Self::start_replace_range(call, args, projector)? {
            return Ok(Some(work));
        }
        if let Some(work) = Self::start_grapheme(call, args, projector)? {
            return Ok(Some(work));
        }
        if let Some(work) = Self::start_scalar(call, args, projector)? {
            return Ok(Some(work));
        }
        if let Some(work) = Self::start_byte_range(call, args, projector)? {
            return Ok(Some(work));
        }
        if let Some(work) = Self::start_ascii_eq(call, args, projector)? {
            return Ok(Some(work));
        }
        if let Some(work) = Self::start_integer_parse(call, args, projector)? {
            return Ok(Some(work));
        }
        if let Some(work) = Self::start_float_parse(call, args, projector)? {
            return Ok(Some(work));
        }
        let (kind, finish) = match call.intrinsic.0 {
            etas_std::intrinsic::pure::TEXT_CONTAINS => {
                (TextSearchKind::Contains, TextSearchFinish::Query)
            }
            etas_std::intrinsic::pure::TEXT_STARTS_WITH => {
                (TextSearchKind::StartsWith, TextSearchFinish::Query)
            }
            etas_std::intrinsic::pure::TEXT_ENDS_WITH => {
                (TextSearchKind::EndsWith, TextSearchFinish::Query)
            }
            etas_std::intrinsic::pure::TEXT_STRIP_PREFIX => {
                (TextSearchKind::StartsWith, TextSearchFinish::StripPrefix)
            }
            etas_std::intrinsic::pure::TEXT_STRIP_SUFFIX => {
                (TextSearchKind::EndsWith, TextSearchFinish::StripSuffix)
            }
            etas_std::intrinsic::pure::TEXT_FIND => (TextSearchKind::Find, TextSearchFinish::Query),
            etas_std::intrinsic::pure::TEXT_RFIND => {
                (TextSearchKind::Rfind, TextSearchFinish::Query)
            }
            etas_std::intrinsic::pure::TEXT_COUNT => {
                (TextSearchKind::Count, TextSearchFinish::Query)
            }
            etas_std::intrinsic::pure::TEXT_FIND_FROM => {
                if args.len() != 3 {
                    return Err(AdapterError::Arity {
                        expected: 3,
                        actual: args.len(),
                    });
                }
                let index = super::abi::input::into_builtin_for_type(
                    args[2].clone(),
                    call.parameter_types[2],
                    projector,
                )?;
                let start = etas_builtin::text::bounds::position(&index)
                    .map_err(|failure| AdapterError::Builtin(BuiltinError::Text(failure)))?;
                (TextSearchKind::FindFrom { start }, TextSearchFinish::Query)
            }
            _ => return Ok(None),
        };
        let expected = if matches!(kind, TextSearchKind::FindFrom { .. }) {
            3
        } else {
            2
        };
        if args.len() != expected {
            return Err(AdapterError::Arity {
                expected,
                actual: args.len(),
            });
        }
        let source =
            borrowed::shared_string_for_type(&args[0], call.parameter_types[0], projector)?.clone();
        let pattern =
            borrowed::shared_string_for_type(&args[1], call.parameter_types[1], projector)?.clone();
        Ok(Some(Self {
            call: NativeWorkOwner::Intrinsic(call.clone()),
            state: TextWorkState::Search {
                source_cursor: source.owned_cursor(),
                pattern_cursor: pattern.owned_cursor(),
                projection: source.owned_cursor(),
                search: TextSearchWork::new(kind, pattern.len()),
                source,
                pattern,
                pattern_ready: false,
                finish,
                selected: None,
                output: TextOutputWork::default(),
            },
        }))
    }

    fn start_repeat(
        call: &CheckedPureIntrinsicCall,
        args: &[InterpValue],
        projector: &PureAbiProjector,
    ) -> Result<Option<Self>, AdapterError> {
        if call.intrinsic.0 != etas_std::intrinsic::pure::TEXT_REPEAT {
            return Ok(None);
        }
        if args.len() != 2 {
            return Err(AdapterError::Arity {
                expected: 2,
                actual: args.len(),
            });
        }
        let source =
            borrowed::shared_string_for_type(&args[0], call.parameter_types[0], projector)?.clone();
        let count = super::abi::input::into_builtin_for_type(
            args[1].clone(),
            call.parameter_types[1],
            projector,
        )?;
        let count = etas_builtin::text::bounds::position(&count)
            .map_err(|failure| AdapterError::Builtin(BuiltinError::Text(failure)))?;
        let total_bytes = source.len().checked_mul(count).ok_or({
            AdapterError::Builtin(BuiltinError::Text(
                etas_builtin::TextFailure::LengthOverflow,
            ))
        })?;
        Ok(Some(Self {
            call: NativeWorkOwner::Intrinsic(call.clone()),
            state: TextWorkState::Repeat {
                cursor: source.owned_cursor(),
                source,
                remaining: count,
                total_bytes,
                output: TextOutputWork::default(),
            },
        }))
    }

    fn start_collect(
        call: &CheckedPureIntrinsicCall,
        args: &[InterpValue],
        projector: &PureAbiProjector,
    ) -> Result<Option<Self>, AdapterError> {
        use etas_builtin::text::iteration::{TextSequenceKind, TextSequencePlan};
        use etas_std::intrinsic::pure;

        let kind = match call.intrinsic.0 {
            pure::TEXT_SPLIT => TextCollectKind::Split,
            pure::TEXT_SPLITN => {
                if args.len() != 3 {
                    return Err(AdapterError::Arity {
                        expected: 3,
                        actual: args.len(),
                    });
                }
                let count = super::abi::input::into_builtin_for_type(
                    args[2].clone(),
                    call.parameter_types[2],
                    projector,
                )?;
                let count = etas_builtin::text::bounds::position(&count)
                    .map_err(|failure| AdapterError::Builtin(BuiltinError::Text(failure)))?;
                TextCollectKind::SplitN(count)
            }
            pure::TEXT_SPLIT_ONCE => TextCollectKind::SplitOnce,
            pure::TEXT_RSPLIT_ONCE => TextCollectKind::RsplitOnce,
            _ => return Ok(None),
        };
        let expected = if matches!(kind, TextCollectKind::SplitN(_)) {
            3
        } else {
            2
        };
        if args.len() != expected {
            return Err(AdapterError::Arity {
                expected,
                actual: args.len(),
            });
        }
        let source =
            borrowed::shared_string_for_type(&args[0], call.parameter_types[0], projector)?.clone();
        let separator =
            borrowed::shared_string_for_type(&args[1], call.parameter_types[1], projector)?.clone();
        let sequence = crate::value::iteration::TextSequenceValue::new(
            source,
            TextSequencePlan::new(TextSequenceKind::Split {
                separator: separator.as_str().to_owned(),
            }),
            call.parameter_types[0],
        )
        .map_err(AdapterError::UnsupportedValue)?;
        let source = IterationSource::new(InterpValue::TextSequence(sequence))
            .map_err(AdapterError::UnsupportedValue)?;
        Ok(Some(Self {
            call: NativeWorkOwner::Intrinsic(call.clone()),
            state: TextWorkState::Collect {
                source: Box::new(source),
                separator,
                index: 0,
                values: Vec::new(),
                kind,
                reduction: None,
            },
        }))
    }

    fn start_replace(
        call: &CheckedPureIntrinsicCall,
        args: &[InterpValue],
        projector: &PureAbiProjector,
    ) -> Result<Option<Self>, AdapterError> {
        use etas_builtin::text::iteration::{TextSequenceKind, TextSequencePlan};
        use etas_std::intrinsic::pure;

        let limit = match call.intrinsic.0 {
            pure::TEXT_REPLACE => None,
            pure::TEXT_REPLACE_N => {
                if args.len() != 4 {
                    return Err(AdapterError::Arity {
                        expected: 4,
                        actual: args.len(),
                    });
                }
                let limit = super::abi::input::into_builtin_for_type(
                    args[3].clone(),
                    call.parameter_types[3],
                    projector,
                )?;
                Some(
                    etas_builtin::text::bounds::position(&limit)
                        .map_err(|failure| AdapterError::Builtin(BuiltinError::Text(failure)))?,
                )
            }
            _ => return Ok(None),
        };
        let expected = if limit.is_some() { 4 } else { 3 };
        if args.len() != expected {
            return Err(AdapterError::Arity {
                expected,
                actual: args.len(),
            });
        }
        let source =
            borrowed::shared_string_for_type(&args[0], call.parameter_types[0], projector)?.clone();
        let pattern =
            borrowed::shared_string_for_type(&args[1], call.parameter_types[1], projector)?.clone();
        let replacement =
            borrowed::shared_string_for_type(&args[2], call.parameter_types[2], projector)?.clone();
        let sequence = crate::value::iteration::TextSequenceValue::new(
            source,
            TextSequencePlan::new(TextSequenceKind::Split {
                separator: pattern.as_str().to_owned(),
            }),
            call.parameter_types[0],
        )
        .map_err(AdapterError::UnsupportedValue)?;
        Ok(Some(Self {
            call: NativeWorkOwner::Intrinsic(call.clone()),
            state: TextWorkState::Replace {
                source: Box::new(
                    IterationSource::new(InterpValue::TextSequence(sequence))
                        .map_err(AdapterError::UnsupportedValue)?,
                ),
                pattern,
                replacement,
                limit,
                index: 0,
                output: None,
            },
        }))
    }

    fn start_replace_range(
        call: &CheckedPureIntrinsicCall,
        args: &[InterpValue],
        projector: &PureAbiProjector,
    ) -> Result<Option<Self>, AdapterError> {
        if call.intrinsic.0 != etas_std::intrinsic::pure::TEXT_REPLACE_RANGE {
            return Ok(None);
        }
        if args.len() != 3 {
            return Err(AdapterError::Arity {
                expected: 3,
                actual: args.len(),
            });
        }
        let source =
            borrowed::shared_string_for_type(&args[0], call.parameter_types[0], projector)?.clone();
        let range = super::abi::input::into_builtin_for_type(
            args[1].clone(),
            call.parameter_types[1],
            projector,
        )?;
        let range = etas_builtin::text::bounds::interval(&range, source.len())
            .map_err(|failure| AdapterError::Builtin(BuiltinError::Text(failure)))?;
        OwnedTextCursor::at(&source, range.start).map_err(|_| {
            AdapterError::Builtin(BuiltinError::Text(
                etas_builtin::TextFailure::NotCharBoundary,
            ))
        })?;
        OwnedTextCursor::at(&source, range.end).map_err(|_| {
            AdapterError::Builtin(BuiltinError::Text(
                etas_builtin::TextFailure::NotCharBoundary,
            ))
        })?;
        let replacement =
            borrowed::shared_string_for_type(&args[2], call.parameter_types[2], projector)?.clone();
        let total_bytes = source
            .len()
            .checked_sub(range.len())
            .and_then(|bytes| bytes.checked_add(replacement.len()))
            .ok_or({
                AdapterError::Builtin(BuiltinError::Text(
                    etas_builtin::TextFailure::LengthOverflow,
                ))
            })?;
        Ok(Some(Self {
            call: NativeWorkOwner::Intrinsic(call.clone()),
            state: TextWorkState::ReplaceRange {
                source_cursor: source.owned_cursor(),
                replacement_cursor: replacement.owned_cursor(),
                source,
                range,
                replacement,
                phase: ReplaceRangePhase::Prefix,
                total_bytes,
                output: TextOutputWork::default(),
            },
        }))
    }

    fn start_grapheme(
        call: &CheckedPureIntrinsicCall,
        args: &[InterpValue],
        projector: &PureAbiProjector,
    ) -> Result<Option<Self>, AdapterError> {
        use etas_builtin::text::iteration::{
            TextCursorState, TextSequenceKind, TextSequencePlan, TextSequenceWork,
        };
        use etas_std::intrinsic::pure;

        let requested = match call.intrinsic.0 {
            pure::TEXT_GRAPHEME_COUNT => {
                if args.len() != 1 {
                    return Err(AdapterError::Arity {
                        expected: 1,
                        actual: args.len(),
                    });
                }
                None
            }
            pure::TEXT_GRAPHEME_SLICE => {
                if args.len() != 2 {
                    return Err(AdapterError::Arity {
                        expected: 2,
                        actual: args.len(),
                    });
                }
                let range = super::abi::input::into_builtin_for_type(
                    args[1].clone(),
                    call.parameter_types[1],
                    projector,
                )?;
                Some(
                    etas_builtin::text::bounds::requested_interval(&range)
                        .map_err(|failure| AdapterError::Builtin(BuiltinError::Text(failure)))?,
                )
            }
            _ => return Ok(None),
        };
        let source =
            borrowed::shared_string_for_type(&args[0], call.parameter_types[0], projector)?.clone();
        let selected_start = requested
            .as_ref()
            .is_some_and(|range| range.start == 0)
            .then_some(0);
        let selected_end = requested
            .as_ref()
            .is_some_and(|range| range.end == 0)
            .then_some(0);
        Ok(Some(Self {
            call: NativeWorkOwner::Intrinsic(call.clone()),
            state: TextWorkState::Grapheme {
                scan: source.owned_cursor(),
                projection: source.owned_cursor(),
                work: TextSequenceWork::new(
                    TextSequencePlan::new(TextSequenceKind::Graphemes),
                    TextCursorState::default(),
                )
                .map_err(|failure| AdapterError::Builtin(BuiltinError::Text(failure)))?,
                source,
                ordinal: 0,
                requested,
                selected_start,
                selected_end,
                output: TextOutputWork::default(),
            },
        }))
    }

    fn start_scalar(
        call: &CheckedPureIntrinsicCall,
        args: &[InterpValue],
        projector: &PureAbiProjector,
    ) -> Result<Option<Self>, AdapterError> {
        use etas_builtin::text::iteration::{
            TextCursorState, TextSequenceKind, TextSequencePlan, TextSequenceWork,
        };
        use etas_std::intrinsic::pure;

        let operation = match call.intrinsic.0 {
            pure::TEXT_CHAR_GET => {
                if args.len() != 2 {
                    return Err(AdapterError::Arity {
                        expected: 2,
                        actual: args.len(),
                    });
                }
                let index = super::abi::input::into_builtin_for_type(
                    args[1].clone(),
                    call.parameter_types[1],
                    projector,
                )?;
                TextScalarOperation::Get {
                    target: etas_builtin::text::bounds::position(&index).ok(),
                }
            }
            pure::TEXT_CHAR_SLICE | pure::TEXT_GET_CHAR_SLICE => {
                if args.len() != 2 {
                    return Err(AdapterError::Arity {
                        expected: 2,
                        actual: args.len(),
                    });
                }
                let range = super::abi::input::into_builtin_for_type(
                    args[1].clone(),
                    call.parameter_types[1],
                    projector,
                )?;
                let optional = call.intrinsic.0 == pure::TEXT_GET_CHAR_SLICE;
                let requested = match etas_builtin::text::bounds::requested_interval(&range) {
                    Ok(range) => Some(range),
                    Err(_) if optional => None,
                    Err(failure) => {
                        return Err(AdapterError::Builtin(BuiltinError::Text(failure)));
                    }
                };
                TextScalarOperation::Slice {
                    requested,
                    optional,
                }
            }
            _ => return Ok(None),
        };
        let source =
            borrowed::shared_string_for_type(&args[0], call.parameter_types[0], projector)?.clone();
        let (selected_start, selected_end) = match &operation {
            TextScalarOperation::Slice {
                requested: Some(requested),
                ..
            } => (
                (requested.start == 0).then_some(0),
                (requested.end == 0).then_some(0),
            ),
            _ => (None, None),
        };
        Ok(Some(Self {
            call: NativeWorkOwner::Intrinsic(call.clone()),
            state: TextWorkState::Scalar {
                scan: source.owned_cursor(),
                projection: source.owned_cursor(),
                work: TextSequenceWork::new(
                    TextSequencePlan::new(TextSequenceKind::Chars),
                    TextCursorState::default(),
                )
                .map_err(|failure| AdapterError::Builtin(BuiltinError::Text(failure)))?,
                source,
                ordinal: 0,
                operation,
                selected_start,
                selected_end,
                output: TextOutputWork::default(),
            },
        }))
    }

    fn start_byte_range(
        call: &CheckedPureIntrinsicCall,
        args: &[InterpValue],
        projector: &PureAbiProjector,
    ) -> Result<Option<Self>, AdapterError> {
        use etas_builtin::text::iteration::{
            TextCursorState, TextSequenceKind, TextSequencePlan, TextSequenceWork,
        };
        use etas_std::intrinsic::pure;

        if !matches!(
            call.intrinsic.0,
            pure::TEXT_BYTE_SLICE | pure::TEXT_GET_BYTE_SLICE | pure::TEXT_IS_CHAR_BOUNDARY
        ) {
            return Ok(None);
        }
        if args.len() != 2 {
            return Err(AdapterError::Arity {
                expected: 2,
                actual: args.len(),
            });
        }
        let source =
            borrowed::shared_string_for_type(&args[0], call.parameter_types[0], projector)?.clone();
        let argument = super::abi::input::into_builtin_for_type(
            args[1].clone(),
            call.parameter_types[1],
            projector,
        )?;
        let operation = match call.intrinsic.0 {
            pure::TEXT_IS_CHAR_BOUNDARY => TextByteOperation::IsBoundary {
                target: etas_builtin::text::bounds::position(&argument)
                    .ok()
                    .filter(|target| *target <= source.len()),
            },
            pure::TEXT_BYTE_SLICE | pure::TEXT_GET_BYTE_SLICE => {
                let optional = call.intrinsic.0 == pure::TEXT_GET_BYTE_SLICE;
                let range = match etas_builtin::text::bounds::interval(&argument, source.len()) {
                    Ok(range) => Some(range),
                    Err(_) if optional => None,
                    Err(failure) => {
                        return Err(AdapterError::Builtin(BuiltinError::Text(failure)));
                    }
                };
                let start_valid = range.as_ref().is_some_and(|range| range.start == 0);
                let end_valid = range.as_ref().is_some_and(|range| range.end == 0);
                TextByteOperation::Slice {
                    range,
                    optional,
                    start_valid,
                    end_valid,
                }
            }
            _ => unreachable!(),
        };
        Ok(Some(Self {
            call: NativeWorkOwner::Intrinsic(call.clone()),
            state: TextWorkState::ByteRange {
                scan: source.owned_cursor(),
                projection: source.owned_cursor(),
                work: TextSequenceWork::new(
                    TextSequencePlan::new(TextSequenceKind::Chars),
                    TextCursorState::default(),
                )
                .map_err(|failure| AdapterError::Builtin(BuiltinError::Text(failure)))?,
                source,
                operation,
                output: TextOutputWork::default(),
            },
        }))
    }

    fn start_ascii_eq(
        call: &CheckedPureIntrinsicCall,
        args: &[InterpValue],
        projector: &PureAbiProjector,
    ) -> Result<Option<Self>, AdapterError> {
        if call.intrinsic.0 != etas_std::intrinsic::pure::TEXT_EQ_IGNORE_ASCII_CASE {
            return Ok(None);
        }
        if args.len() != 2 {
            return Err(AdapterError::Arity {
                expected: 2,
                actual: args.len(),
            });
        }
        let left =
            borrowed::shared_string_for_type(&args[0], call.parameter_types[0], projector)?.clone();
        let right =
            borrowed::shared_string_for_type(&args[1], call.parameter_types[1], projector)?.clone();
        Ok(Some(Self {
            call: NativeWorkOwner::Intrinsic(call.clone()),
            state: TextWorkState::AsciiEq {
                left_cursor: left.owned_cursor(),
                right_cursor: right.owned_cursor(),
                left,
                right,
            },
        }))
    }

    fn start_integer_parse(
        call: &CheckedPureIntrinsicCall,
        args: &[InterpValue],
        projector: &PureAbiProjector,
    ) -> Result<Option<Self>, AdapterError> {
        let Some(kind) = etas_builtin::text::parse::TextIntegerKind::for_intrinsic(call.intrinsic)
        else {
            return Ok(None);
        };
        if args.len() != 1 {
            return Err(AdapterError::Arity {
                expected: 1,
                actual: args.len(),
            });
        }
        let source =
            borrowed::shared_string_for_type(&args[0], call.parameter_types[0], projector)?.clone();
        Ok(Some(Self {
            call: NativeWorkOwner::Intrinsic(call.clone()),
            state: TextWorkState::IntegerParse {
                cursor: source.owned_cursor(),
                source,
                kind,
                work: etas_builtin::text::parse::TextIntegerParseWork::default(),
            },
        }))
    }

    fn start_float_parse(
        call: &CheckedPureIntrinsicCall,
        args: &[InterpValue],
        projector: &PureAbiProjector,
    ) -> Result<Option<Self>, AdapterError> {
        let Some(kind) = etas_builtin::text::parse::TextFloatKind::for_intrinsic(call.intrinsic)
        else {
            return Ok(None);
        };
        if args.len() != 1 {
            return Err(AdapterError::Arity {
                expected: 1,
                actual: args.len(),
            });
        }
        let source =
            borrowed::shared_string_for_type(&args[0], call.parameter_types[0], projector)?.clone();
        Ok(Some(Self {
            call: NativeWorkOwner::Intrinsic(call.clone()),
            state: TextWorkState::FloatParse {
                cursor: source.owned_cursor(),
                source,
                kind,
                work: etas_builtin::text::parse::TextFloatParseWork::default(),
            },
        }))
    }

    fn start_join(
        call: &CheckedPureIntrinsicCall,
        args: &[InterpValue],
        projector: &PureAbiProjector,
    ) -> Result<Option<Self>, AdapterError> {
        let separator = match call.intrinsic.0 {
            etas_std::intrinsic::pure::TEXT_CONCAT => {
                if args.len() != 1 {
                    return Err(AdapterError::Arity {
                        expected: 1,
                        actual: args.len(),
                    });
                }
                StringValue::default()
            }
            etas_std::intrinsic::pure::TEXT_JOIN => {
                if args.len() != 2 {
                    return Err(AdapterError::Arity {
                        expected: 2,
                        actual: args.len(),
                    });
                }
                borrowed::shared_string_for_type(&args[1], call.parameter_types[1], projector)?
                    .clone()
            }
            _ => return Ok(None),
        };
        let (parts, parts_type) =
            borrowed::representation_for_type(&args[0], call.parameter_types[0], projector)?;
        let (Some(super::abi::AbiShape::Array(item_type)), InterpValue::Array(parts)) =
            (projector.shape(parts_type), parts)
        else {
            return Err(super::abi::input::type_mismatch(parts_type, parts));
        };
        Ok(Some(Self {
            call: NativeWorkOwner::Intrinsic(call.clone()),
            state: TextWorkState::Join {
                parts: parts.clone(),
                item_type: *item_type,
                separator,
                phase: JoinPhase::Size { index: 0, bytes: 0 },
                output: TextOutputWork::default(),
            },
        }))
    }

    fn start_case(
        call: &CheckedPureIntrinsicCall,
        args: &[InterpValue],
        projector: &PureAbiProjector,
    ) -> Result<Option<Self>, AdapterError> {
        let mapping = match call.intrinsic.0 {
            etas_std::intrinsic::pure::TEXT_LOWERCASE => CaseMapping::Lowercase,
            etas_std::intrinsic::pure::TEXT_UPPERCASE => CaseMapping::Uppercase,
            etas_std::intrinsic::pure::TEXT_ASCII_LOWERCASE => CaseMapping::AsciiLowercase,
            etas_std::intrinsic::pure::TEXT_ASCII_UPPERCASE => CaseMapping::AsciiUppercase,
            _ => return Ok(None),
        };
        if args.len() != 1 {
            return Err(AdapterError::Arity {
                expected: 1,
                actual: args.len(),
            });
        }
        let source =
            borrowed::shared_string_for_type(&args[0], call.parameter_types[0], projector)?.clone();
        Ok(Some(Self {
            call: NativeWorkOwner::Intrinsic(call.clone()),
            state: TextWorkState::Case {
                cursor: source.owned_cursor(),
                source,
                work: TextCaseWork::new(mapping),
            },
        }))
    }

    fn start_trim(
        call: &CheckedPureIntrinsicCall,
        args: &[InterpValue],
        projector: &PureAbiProjector,
    ) -> Result<Option<Self>, AdapterError> {
        let kind = match call.intrinsic.0 {
            etas_std::intrinsic::pure::TEXT_TRIM => TextTrimKind::Both,
            etas_std::intrinsic::pure::TEXT_TRIM_START => TextTrimKind::Start,
            etas_std::intrinsic::pure::TEXT_TRIM_END => TextTrimKind::End,
            _ => return Ok(None),
        };
        if args.len() != 1 {
            return Err(AdapterError::Arity {
                expected: 1,
                actual: args.len(),
            });
        }
        let source =
            borrowed::shared_string_for_type(&args[0], call.parameter_types[0], projector)?.clone();
        Ok(Some(Self {
            call: NativeWorkOwner::Intrinsic(call.clone()),
            state: TextWorkState::Trim {
                cursor: source.owned_cursor(),
                source,
                work: TextTrimWork::new(kind),
            },
        }))
    }

    fn start_casefold(
        call: &CheckedPureIntrinsicCall,
        args: &[InterpValue],
        projector: &PureAbiProjector,
    ) -> Result<Option<Self>, AdapterError> {
        if call.intrinsic.0 != etas_std::intrinsic::pure::TEXT_CASEFOLD {
            return Ok(None);
        }
        if args.len() != 1 {
            return Err(AdapterError::Arity {
                expected: 1,
                actual: args.len(),
            });
        }
        let source =
            borrowed::shared_string_for_type(&args[0], call.parameter_types[0], projector)?.clone();
        Ok(Some(Self {
            call: NativeWorkOwner::Intrinsic(call.clone()),
            state: TextWorkState::Casefold {
                cursor: source.owned_cursor(),
                source,
                work: TextCasefoldWork::default(),
            },
        }))
    }

    fn start_normalize(
        call: &CheckedPureIntrinsicCall,
        args: &[InterpValue],
        projector: &PureAbiProjector,
    ) -> Result<Option<Self>, AdapterError> {
        let query = match call.intrinsic.0 {
            etas_std::intrinsic::pure::TEXT_NORMALIZE => false,
            etas_std::intrinsic::pure::TEXT_IS_NORMALIZED => true,
            _ => return Ok(None),
        };
        if args.len() != 2 {
            return Err(AdapterError::Arity {
                expected: 2,
                actual: args.len(),
            });
        }
        let source =
            borrowed::shared_string_for_type(&args[0], call.parameter_types[0], projector)?.clone();
        let form = super::abi::input::into_builtin_for_type(
            args[1].clone(),
            call.parameter_types[1],
            projector,
        )?;
        let form = NormalizationForm::from_builtin(&form).map_err(AdapterError::Builtin)?;
        Ok(Some(Self {
            call: NativeWorkOwner::Intrinsic(call.clone()),
            state: TextWorkState::Normalize {
                cursor: source.owned_cursor(),
                source,
                work: TextNormalizeWork::new(form),
                query,
                comparison: None,
            },
        }))
    }

    fn start_reverse(
        call: &CheckedPureIntrinsicCall,
        args: &[InterpValue],
        projector: &PureAbiProjector,
    ) -> Result<Option<Self>, AdapterError> {
        let kind = match call.intrinsic.0 {
            etas_std::intrinsic::pure::TEXT_REVERSE_CHARS => TextReverseKind::Chars,
            etas_std::intrinsic::pure::TEXT_REVERSE_GRAPHEMES => TextReverseKind::Graphemes,
            _ => return Ok(None),
        };
        if args.len() != 1 {
            return Err(AdapterError::Arity {
                expected: 1,
                actual: args.len(),
            });
        }
        let source =
            borrowed::shared_string_for_type(&args[0], call.parameter_types[0], projector)?.clone();
        Ok(Some(Self {
            call: NativeWorkOwner::Intrinsic(call.clone()),
            state: TextWorkState::Reverse {
                cursor: source.owned_cursor(),
                source,
                work: TextReverseWork::new(kind),
            },
        }))
    }

    fn start_pad(
        call: &CheckedPureIntrinsicCall,
        args: &[InterpValue],
        projector: &PureAbiProjector,
    ) -> Result<Option<Self>, AdapterError> {
        let start = match call.intrinsic.0 {
            etas_std::intrinsic::pure::TEXT_PAD_START => true,
            etas_std::intrinsic::pure::TEXT_PAD_END => false,
            _ => return Ok(None),
        };
        if args.len() != 3 {
            return Err(AdapterError::Arity {
                expected: 3,
                actual: args.len(),
            });
        }
        let source =
            borrowed::shared_string_for_type(&args[0], call.parameter_types[0], projector)?.clone();
        let width = super::abi::input::into_builtin_for_type(
            args[1].clone(),
            call.parameter_types[1],
            projector,
        )?;
        let width = etas_builtin::text::bounds::position(&width)
            .map_err(|failure| AdapterError::Builtin(BuiltinError::Text(failure)))?;
        let fill = super::abi::input::into_builtin_for_type(
            args[2].clone(),
            call.parameter_types[2],
            projector,
        )?;
        let etas_builtin::BuiltinValue::Char(fill) = fill else {
            return Err(AdapterError::Builtin(BuiltinError::TypeMismatch {
                expected: etas_builtin::BuiltinTypeTag::Char,
                actual: fill.type_tag(),
            }));
        };
        Ok(Some(Self {
            call: NativeWorkOwner::Intrinsic(call.clone()),
            state: TextWorkState::Pad {
                cursor: source.owned_cursor(),
                source,
                width,
                fill,
                start,
                phase: PadPhase::Count { scalars: 0 },
                output: TextOutputWork::default(),
            },
        }))
    }

    fn start_format(
        call: &CheckedPureIntrinsicCall,
        args: &[InterpValue],
        projector: &PureAbiProjector,
    ) -> Result<Option<Self>, AdapterError> {
        if call.intrinsic.0 != etas_std::intrinsic::pure::TEXT_FORMAT {
            return Ok(None);
        }
        if args.len() != 2 {
            return Err(AdapterError::Arity {
                expected: 2,
                actual: args.len(),
            });
        }
        let template =
            borrowed::shared_string_for_type(&args[0], call.parameter_types[0], projector)?.clone();
        let (arguments, arguments_type) =
            borrowed::representation_for_type(&args[1], call.parameter_types[1], projector)?;
        let (Some(super::abi::AbiShape::Array(item_type)), InterpValue::Array(arguments)) =
            (projector.shape(arguments_type), arguments)
        else {
            return Err(super::abi::input::type_mismatch(arguments_type, arguments));
        };
        Ok(Some(Self {
            call: NativeWorkOwner::Intrinsic(call.clone()),
            state: TextWorkState::Format {
                template_cursor: template.owned_cursor(),
                template,
                arguments: arguments.clone(),
                item_type: *item_type,
                argument: None,
                work: TextFormatWork::default(),
            },
        }))
    }

    pub(crate) fn poll(
        &mut self,
        projector: &PureAbiProjector,
    ) -> Result<NativeWorkPoll, AdapterError> {
        let output_remaining = MAX_NATIVE_OUTPUT_BYTES.saturating_sub(self.output_reserved());
        let scratch_remaining = MAX_NATIVE_SCRATCH_BYTES.saturating_sub(self.scratch_reserved());
        let mut control = WorkControl::new(WORK_QUANTUM, output_remaining, scratch_remaining);
        match &mut self.state {
            TextWorkState::IntegerParse {
                source,
                cursor,
                kind,
                work,
            } => loop {
                let (consumed, poll) = if let Some(piece) = cursor.next_piece() {
                    let text = piece.as_str();
                    let end_of_input = cursor.position() + text.len() == source.len();
                    work.step(text, end_of_input, &mut control)
                } else {
                    work.step("", true, &mut control)
                };
                cursor.advance(consumed);
                match poll {
                    WorkPoll::Pending => {
                        if consumed == 0 || control.work_remaining() == 0 {
                            return Ok(NativeWorkPoll::Pending);
                        }
                    }
                    WorkPoll::Ready(parsed) => {
                        let value = parsed.and_then(|parsed| kind.finish(parsed));
                        return from_builtin_for_type(
                            etas_builtin::text::parse::result(value),
                            self.call.result_type,
                            projector,
                        )
                        .map(NativeWorkPoll::Ready);
                    }
                }
            },
            TextWorkState::FloatParse {
                source,
                cursor,
                kind,
                work,
            } => loop {
                let (consumed, poll) = if let Some(piece) = cursor.next_piece() {
                    let text = piece.as_str();
                    let end_of_input = cursor.position() + text.len() == source.len();
                    work.step(text, end_of_input, &mut control)
                } else {
                    work.step("", true, &mut control)
                };
                cursor.advance(consumed);
                match poll {
                    WorkPoll::Pending => {
                        if consumed == 0 || control.work_remaining() == 0 {
                            return Ok(NativeWorkPoll::Pending);
                        }
                    }
                    WorkPoll::Ready(parsed) => {
                        let value = parsed.and_then(|parsed| kind.finish(parsed));
                        return from_builtin_for_type(
                            etas_builtin::text::parse::result(value),
                            self.call.result_type,
                            projector,
                        )
                        .map(NativeWorkPoll::Ready);
                    }
                }
            },
            TextWorkState::Query {
                source,
                cursor,
                query,
            } => loop {
                let Some(piece) = cursor.next_piece() else {
                    let (_, result) = query
                        .step("", true, &mut control)
                        .map_err(AdapterError::Builtin)?;
                    let WorkPoll::Ready(value) = result else {
                        return Ok(NativeWorkPoll::Pending);
                    };
                    return from_builtin_for_type(value, self.call.result_type, projector)
                        .map(NativeWorkPoll::Ready);
                };
                let text = piece.as_str();
                let end_of_input = cursor.position() + text.len() == source.len();
                let (consumed, result) = query
                    .step(text, end_of_input, &mut control)
                    .map_err(AdapterError::Builtin)?;
                cursor.advance(consumed);
                if let WorkPoll::Ready(value) = result {
                    return from_builtin_for_type(value, self.call.result_type, projector)
                        .map(NativeWorkPoll::Ready);
                }
                if consumed == 0 || control.work_remaining() == 0 {
                    return Ok(NativeWorkPoll::Pending);
                }
                if cursor.is_finished() {
                    continue;
                }
            },
            TextWorkState::Search {
                source,
                source_cursor,
                pattern,
                pattern_cursor,
                pattern_ready,
                search,
                finish,
                selected,
                projection,
                output,
            } => {
                if let Some(range) = selected.clone() {
                    output
                        .reserve_exact(range.len(), &mut control)
                        .map_err(AdapterError::Builtin)?;
                    skip_until(projection, range.start, &mut control)?;
                    if projection.position() != range.start {
                        return Ok(NativeWorkPoll::Pending);
                    }
                    append_until(projection, range.end, output, &mut control)?;
                    if projection.position() != range.end {
                        return Ok(NativeWorkPoll::Pending);
                    }
                    return super::abi::text::from_optional_text_value(
                        Some(output.take().into()),
                        self.call.result_type,
                        projector,
                    )
                    .map(NativeWorkPoll::Ready);
                }
                if !*pattern_ready {
                    loop {
                        let Some(piece) = pattern_cursor.next_piece() else {
                            let (_, poll) = search
                                .pattern_step("", true, &mut control)
                                .map_err(AdapterError::Builtin)?;
                            if poll == WorkPoll::Ready(()) {
                                *pattern_ready = true;
                                break;
                            }
                            return Ok(NativeWorkPoll::Pending);
                        };
                        let text = piece.as_str();
                        let end_of_input = pattern_cursor.position() + text.len() == pattern.len();
                        let (consumed, poll) = search
                            .pattern_step(text, end_of_input, &mut control)
                            .map_err(AdapterError::Builtin)?;
                        pattern_cursor.advance(consumed);
                        if poll == WorkPoll::Ready(()) {
                            *pattern_ready = true;
                            break;
                        }
                        if consumed == 0 || control.work_remaining() == 0 {
                            return Ok(NativeWorkPoll::Pending);
                        }
                    }
                }
                loop {
                    let Some(piece) = source_cursor.next_piece() else {
                        let (_, result) = search
                            .source_step("", true, &mut control)
                            .map_err(AdapterError::Builtin)?;
                        let WorkPoll::Ready(value) = result else {
                            return Ok(NativeWorkPoll::Pending);
                        };
                        match search_completion(*finish, value, source.len(), pattern.len())? {
                            SearchCompletion::Query(value) => {
                                return from_builtin_for_type(
                                    value,
                                    self.call.result_type,
                                    projector,
                                )
                                .map(NativeWorkPoll::Ready);
                            }
                            SearchCompletion::Missing => {
                                return super::abi::text::from_optional_text_value(
                                    None,
                                    self.call.result_type,
                                    projector,
                                )
                                .map(NativeWorkPoll::Ready);
                            }
                            SearchCompletion::Range(range) => {
                                *selected = Some(range);
                                return Ok(NativeWorkPoll::Pending);
                            }
                        }
                    };
                    let text = piece.as_str();
                    let end_of_input = source_cursor.position() + text.len() == source.len();
                    let (consumed, result) = search
                        .source_step(text, end_of_input, &mut control)
                        .map_err(AdapterError::Builtin)?;
                    source_cursor.advance(consumed);
                    if let WorkPoll::Ready(value) = result {
                        match search_completion(*finish, value, source.len(), pattern.len())? {
                            SearchCompletion::Query(value) => {
                                return from_builtin_for_type(
                                    value,
                                    self.call.result_type,
                                    projector,
                                )
                                .map(NativeWorkPoll::Ready);
                            }
                            SearchCompletion::Missing => {
                                return super::abi::text::from_optional_text_value(
                                    None,
                                    self.call.result_type,
                                    projector,
                                )
                                .map(NativeWorkPoll::Ready);
                            }
                            SearchCompletion::Range(range) => {
                                *selected = Some(range);
                                return Ok(NativeWorkPoll::Pending);
                            }
                        }
                    }
                    if consumed == 0 || control.work_remaining() == 0 {
                        return Ok(NativeWorkPoll::Pending);
                    }
                }
            }
            TextWorkState::Repeat {
                source,
                cursor,
                remaining,
                total_bytes,
                output,
            } => {
                output
                    .reserve_exact(*total_bytes, &mut control)
                    .map_err(AdapterError::Builtin)?;
                if *remaining == 0 || source.is_empty() {
                    let value = etas_builtin::BuiltinValue::String(output.take());
                    return from_builtin_for_type(value, self.call.result_type, projector)
                        .map(NativeWorkPoll::Ready);
                }
                loop {
                    let Some(piece) = cursor.next_piece() else {
                        *remaining -= 1;
                        if *remaining == 0 {
                            let value = etas_builtin::BuiltinValue::String(output.take());
                            return from_builtin_for_type(value, self.call.result_type, projector)
                                .map(NativeWorkPoll::Ready);
                        }
                        *cursor = source.owned_cursor();
                        continue;
                    };
                    let consumed = output
                        .append_step(piece.as_str(), &mut control)
                        .map_err(AdapterError::Builtin)?;
                    cursor.advance(consumed);
                    if consumed == 0 || control.work_remaining() == 0 {
                        return Ok(NativeWorkPoll::Pending);
                    }
                }
            }
            TextWorkState::Join {
                parts,
                item_type,
                separator,
                phase,
                output,
            } => loop {
                match phase {
                    JoinPhase::Size { index, bytes } => {
                        while *index < parts.borrow().len() {
                            if control.consume(1).is_err() {
                                return Ok(NativeWorkPoll::Pending);
                            }
                            let part = borrowed::shared_string_for_type(
                                &parts.borrow()[*index],
                                *item_type,
                                projector,
                            )?;
                            *bytes = bytes
                                .checked_add(part.len())
                                .and_then(|bytes| {
                                    bytes.checked_add(if *index == 0 { 0 } else { separator.len() })
                                })
                                .ok_or({
                                    AdapterError::Builtin(BuiltinError::Text(
                                        etas_builtin::TextFailure::LengthOverflow,
                                    ))
                                })?;
                            *index += 1;
                        }
                        output
                            .reserve_exact(*bytes, &mut control)
                            .map_err(AdapterError::Builtin)?;
                        *phase = JoinPhase::Write {
                            index: 0,
                            separator_pending: false,
                            current: None,
                        };
                    }
                    JoinPhase::Write {
                        index,
                        separator_pending,
                        current,
                    } => {
                        if current.is_none() {
                            if *index == parts.borrow().len() {
                                let value = etas_builtin::BuiltinValue::String(output.take());
                                return from_builtin_for_type(
                                    value,
                                    self.call.result_type,
                                    projector,
                                )
                                .map(NativeWorkPoll::Ready);
                            }
                            let source = if *separator_pending {
                                separator.clone()
                            } else {
                                borrowed::shared_string_for_type(
                                    &parts.borrow()[*index],
                                    *item_type,
                                    projector,
                                )?
                                .clone()
                            };
                            *current = Some((source.clone(), source.owned_cursor()));
                        }
                        let (_source, cursor) = current.as_mut().expect("join source prepared");
                        let Some(piece) = cursor.next_piece() else {
                            *current = None;
                            if *separator_pending {
                                *separator_pending = false;
                            } else {
                                *index += 1;
                                *separator_pending = *index < parts.borrow().len();
                            }
                            continue;
                        };
                        let consumed = output
                            .append_step(piece.as_str(), &mut control)
                            .map_err(AdapterError::Builtin)?;
                        cursor.advance(consumed);
                        if consumed == 0 || control.work_remaining() == 0 {
                            return Ok(NativeWorkPoll::Pending);
                        }
                    }
                }
            },
            TextWorkState::Case {
                source,
                cursor,
                work,
            } => loop {
                let Some(piece) = cursor.next_piece() else {
                    let (_, result) = work
                        .step("", true, &mut control)
                        .map_err(AdapterError::Builtin)?;
                    let WorkPoll::Ready(value) = result else {
                        return Ok(NativeWorkPoll::Pending);
                    };
                    return from_builtin_for_type(
                        etas_builtin::BuiltinValue::String(value),
                        self.call.result_type,
                        projector,
                    )
                    .map(NativeWorkPoll::Ready);
                };
                let text = piece.as_str();
                let end_of_input = cursor.position() + text.len() == source.len();
                let (consumed, result) = work
                    .step(text, end_of_input, &mut control)
                    .map_err(AdapterError::Builtin)?;
                cursor.advance(consumed);
                if let WorkPoll::Ready(value) = result {
                    return from_builtin_for_type(
                        etas_builtin::BuiltinValue::String(value),
                        self.call.result_type,
                        projector,
                    )
                    .map(NativeWorkPoll::Ready);
                }
                if consumed == 0 || control.work_remaining() == 0 {
                    return Ok(NativeWorkPoll::Pending);
                }
            },
            TextWorkState::Trim {
                source,
                cursor,
                work,
            } => loop {
                let Some(piece) = cursor.next_piece() else {
                    let (_, result) = work.step("", true, &mut control);
                    let WorkPoll::Ready(range) = result else {
                        return Ok(NativeWorkPoll::Pending);
                    };
                    let mut ranges = TextRangeProjector::new(source);
                    return super::abi::text::from_text_range(
                        range,
                        &mut ranges,
                        self.call.result_type,
                        projector,
                    )
                    .map(NativeWorkPoll::Ready);
                };
                let text = piece.as_str();
                let end_of_input = cursor.position() + text.len() == source.len();
                let (consumed, result) = work.step(text, end_of_input, &mut control);
                cursor.advance(consumed);
                if let WorkPoll::Ready(range) = result {
                    let mut ranges = TextRangeProjector::new(source);
                    return super::abi::text::from_text_range(
                        range,
                        &mut ranges,
                        self.call.result_type,
                        projector,
                    )
                    .map(NativeWorkPoll::Ready);
                }
                if consumed == 0 || control.work_remaining() == 0 {
                    return Ok(NativeWorkPoll::Pending);
                }
            },
            TextWorkState::Casefold {
                source,
                cursor,
                work,
            } => loop {
                let Some(piece) = cursor.next_piece() else {
                    let (_, result) = work
                        .step("", true, &mut control)
                        .map_err(AdapterError::Builtin)?;
                    let WorkPoll::Ready(value) = result else {
                        return Ok(NativeWorkPoll::Pending);
                    };
                    return from_builtin_for_type(
                        etas_builtin::BuiltinValue::String(value),
                        self.call.result_type,
                        projector,
                    )
                    .map(NativeWorkPoll::Ready);
                };
                let text = piece.as_str();
                let end_of_input = cursor.position() + text.len() == source.len();
                let (consumed, result) = work
                    .step(text, end_of_input, &mut control)
                    .map_err(AdapterError::Builtin)?;
                cursor.advance(consumed);
                if let WorkPoll::Ready(value) = result {
                    return from_builtin_for_type(
                        etas_builtin::BuiltinValue::String(value),
                        self.call.result_type,
                        projector,
                    )
                    .map(NativeWorkPoll::Ready);
                }
                if consumed == 0 || control.work_remaining() == 0 {
                    return Ok(NativeWorkPoll::Pending);
                }
            },
            TextWorkState::Normalize {
                source,
                cursor,
                work,
                query,
                comparison,
            } => loop {
                if let Some((normalized, offset)) = comparison {
                    let Some(piece) = cursor.next_piece() else {
                        let equal = *offset == normalized.len();
                        return from_builtin_for_type(
                            etas_builtin::BuiltinValue::Bool(equal),
                            self.call.result_type,
                            projector,
                        )
                        .map(NativeWorkPoll::Ready);
                    };
                    let source_ch = piece.as_str().chars().next().expect("non-empty text piece");
                    let Some(normalized_ch) = normalized[*offset..].chars().next() else {
                        return from_builtin_for_type(
                            etas_builtin::BuiltinValue::Bool(false),
                            self.call.result_type,
                            projector,
                        )
                        .map(NativeWorkPoll::Ready);
                    };
                    let units = source_ch.len_utf8().max(normalized_ch.len_utf8());
                    if control.consume(units).is_err() {
                        return Ok(NativeWorkPoll::Pending);
                    }
                    if source_ch != normalized_ch {
                        return from_builtin_for_type(
                            etas_builtin::BuiltinValue::Bool(false),
                            self.call.result_type,
                            projector,
                        )
                        .map(NativeWorkPoll::Ready);
                    }
                    cursor.advance(source_ch.len_utf8());
                    *offset += normalized_ch.len_utf8();
                    continue;
                }
                let Some(piece) = cursor.next_piece() else {
                    let (_, result) = work
                        .step("", true, &mut control)
                        .map_err(AdapterError::Builtin)?;
                    let WorkPoll::Ready(value) = result else {
                        return Ok(NativeWorkPoll::Pending);
                    };
                    if *query {
                        *cursor = source.owned_cursor();
                        *comparison = Some((value, 0));
                        continue;
                    }
                    return from_builtin_for_type(
                        etas_builtin::BuiltinValue::String(value),
                        self.call.result_type,
                        projector,
                    )
                    .map(NativeWorkPoll::Ready);
                };
                let text = piece.as_str();
                let end_of_input = cursor.position() + text.len() == source.len();
                let (consumed, result) = work
                    .step(text, end_of_input, &mut control)
                    .map_err(AdapterError::Builtin)?;
                cursor.advance(consumed);
                if let WorkPoll::Ready(value) = result {
                    if *query {
                        *cursor = source.owned_cursor();
                        *comparison = Some((value, 0));
                        continue;
                    }
                    return from_builtin_for_type(
                        etas_builtin::BuiltinValue::String(value),
                        self.call.result_type,
                        projector,
                    )
                    .map(NativeWorkPoll::Ready);
                }
                if consumed == 0 || control.work_remaining() == 0 {
                    return Ok(NativeWorkPoll::Pending);
                }
            },
            TextWorkState::Reverse {
                source,
                cursor,
                work,
            } => loop {
                let Some(piece) = cursor.next_piece() else {
                    let (_, result) = work
                        .step("", true, &mut control)
                        .map_err(AdapterError::Builtin)?;
                    let WorkPoll::Ready(value) = result else {
                        return Ok(NativeWorkPoll::Pending);
                    };
                    return from_builtin_for_type(
                        etas_builtin::BuiltinValue::String(value),
                        self.call.result_type,
                        projector,
                    )
                    .map(NativeWorkPoll::Ready);
                };
                let text = piece.as_str();
                let end_of_input = cursor.position() + text.len() == source.len();
                let (consumed, result) = work
                    .step(text, end_of_input, &mut control)
                    .map_err(AdapterError::Builtin)?;
                cursor.advance(consumed);
                if let WorkPoll::Ready(value) = result {
                    return from_builtin_for_type(
                        etas_builtin::BuiltinValue::String(value),
                        self.call.result_type,
                        projector,
                    )
                    .map(NativeWorkPoll::Ready);
                }
                if consumed == 0 || control.work_remaining() == 0 {
                    return Ok(NativeWorkPoll::Pending);
                }
            },
            TextWorkState::Pad {
                source,
                cursor,
                width,
                fill,
                start,
                phase,
                output,
            } => loop {
                match phase {
                    PadPhase::Count { scalars } => {
                        let Some(piece) = cursor.next_piece() else {
                            let padding = width.saturating_sub(*scalars);
                            if padding == 0 {
                                let mut ranges = TextRangeProjector::new(source);
                                return super::abi::text::from_text_range(
                                    0..source.len(),
                                    &mut ranges,
                                    self.call.result_type,
                                    projector,
                                )
                                .map(NativeWorkPoll::Ready);
                            }
                            let total = fill
                                .len_utf8()
                                .checked_mul(padding)
                                .and_then(|bytes| bytes.checked_add(source.len()))
                                .ok_or({
                                    AdapterError::Builtin(BuiltinError::Text(
                                        etas_builtin::TextFailure::LengthOverflow,
                                    ))
                                })?;
                            output
                                .reserve_exact(total, &mut control)
                                .map_err(AdapterError::Builtin)?;
                            *cursor = source.owned_cursor();
                            *phase = if *start {
                                PadPhase::Padding {
                                    remaining: padding,
                                    source_next: true,
                                }
                            } else {
                                PadPhase::Source {
                                    padding_after: padding,
                                }
                            };
                            continue;
                        };
                        let text = piece.as_str();
                        let mut consumed = 0usize;
                        while let Some(ch) = text[consumed..].chars().next() {
                            let units = ch.len_utf8();
                            if control.consume(units).is_err() {
                                break;
                            }
                            *scalars = scalars.checked_add(1).ok_or({
                                AdapterError::Builtin(BuiltinError::Text(
                                    etas_builtin::TextFailure::LengthOverflow,
                                ))
                            })?;
                            consumed += units;
                        }
                        cursor.advance(consumed);
                        if consumed < text.len() || control.work_remaining() == 0 {
                            return Ok(NativeWorkPoll::Pending);
                        }
                    }
                    PadPhase::Padding {
                        remaining,
                        source_next,
                    } => {
                        while *remaining != 0 {
                            if !output
                                .append_char(*fill, &mut control)
                                .map_err(AdapterError::Builtin)?
                            {
                                return Ok(NativeWorkPoll::Pending);
                            }
                            *remaining -= 1;
                        }
                        if *source_next {
                            *phase = PadPhase::Source { padding_after: 0 };
                            continue;
                        }
                        let value = etas_builtin::BuiltinValue::String(output.take());
                        return from_builtin_for_type(value, self.call.result_type, projector)
                            .map(NativeWorkPoll::Ready);
                    }
                    PadPhase::Source { padding_after } => {
                        let Some(piece) = cursor.next_piece() else {
                            if *padding_after != 0 {
                                *phase = PadPhase::Padding {
                                    remaining: *padding_after,
                                    source_next: false,
                                };
                                continue;
                            }
                            let value = etas_builtin::BuiltinValue::String(output.take());
                            return from_builtin_for_type(value, self.call.result_type, projector)
                                .map(NativeWorkPoll::Ready);
                        };
                        let consumed = output
                            .append_step(piece.as_str(), &mut control)
                            .map_err(AdapterError::Builtin)?;
                        cursor.advance(consumed);
                        if consumed == 0 || control.work_remaining() == 0 {
                            return Ok(NativeWorkPoll::Pending);
                        }
                    }
                }
            },
            TextWorkState::Format {
                template,
                template_cursor,
                arguments,
                item_type,
                argument,
                work,
            } => loop {
                if let Some((source, cursor)) = argument {
                    let (consumed, progress) = if let Some(piece) = cursor.next_piece() {
                        let text = piece.as_str();
                        let end = cursor.position() + text.len() == source.len();
                        work.argument_step(text, end, &mut control)
                            .map_err(AdapterError::Builtin)?
                    } else {
                        work.argument_step("", true, &mut control)
                            .map_err(AdapterError::Builtin)?
                    };
                    cursor.advance(consumed);
                    match progress {
                        TextFormatProgress::Pending => {
                            if consumed == 0 || control.work_remaining() == 0 {
                                return Ok(NativeWorkPoll::Pending);
                            }
                        }
                        TextFormatProgress::ArgumentComplete => *argument = None,
                        TextFormatProgress::Argument(_) | TextFormatProgress::Ready(_) => {
                            unreachable!("argument work progress")
                        }
                    }
                    continue;
                }
                let (consumed, progress) = if let Some(piece) = template_cursor.next_piece() {
                    let text = piece.as_str();
                    let end = template_cursor.position() + text.len() == template.len();
                    work.template_step(text, end, &mut control)
                        .map_err(AdapterError::Builtin)?
                } else {
                    work.template_step("", true, &mut control)
                        .map_err(AdapterError::Builtin)?
                };
                template_cursor.advance(consumed);
                match progress {
                    TextFormatProgress::Pending => {
                        if consumed == 0 || control.work_remaining() == 0 {
                            return Ok(NativeWorkPoll::Pending);
                        }
                    }
                    TextFormatProgress::Argument(index) => {
                        let Some(value) = arguments.borrow().get(index) else {
                            let value = etas_builtin::text::parse::result(Err(
                                etas_builtin::TextFailure::InvalidFormat,
                            ));
                            return from_builtin_for_type(value, self.call.result_type, projector)
                                .map(NativeWorkPoll::Ready);
                        };
                        let source =
                            borrowed::shared_string_for_type(value, *item_type, projector)?.clone();
                        *argument = Some((source.clone(), source.owned_cursor()));
                    }
                    TextFormatProgress::Ready(result) => {
                        let value = etas_builtin::text::parse::result(
                            result.map(etas_builtin::BuiltinValue::String),
                        );
                        return from_builtin_for_type(value, self.call.result_type, projector)
                            .map(NativeWorkPoll::Ready);
                    }
                    TextFormatProgress::ArgumentComplete => {
                        unreachable!("template work progress")
                    }
                }
            },
            TextWorkState::Collect {
                source,
                separator,
                index,
                values,
                kind,
                reduction,
            } => match source.poll(*index, &mut control).map_err(|message| {
                AdapterError::UnsupportedValue(format!(
                    "invalid checked text collection: {message}"
                ))
            })? {
                IterationPoll::Pending => Ok(NativeWorkPoll::Pending),
                IterationPoll::Ready(Some(value)) => {
                    values.push(value);
                    *index += 1;
                    Ok(NativeWorkPoll::Pending)
                }
                IterationPoll::Ready(None) => {
                    match poll_text_collection(values, separator, *kind, reduction, &mut control)? {
                        WorkPoll::Pending => Ok(NativeWorkPoll::Pending),
                        WorkPoll::Ready(value) => Ok(NativeWorkPoll::Ready(value)),
                    }
                }
            },
            TextWorkState::Replace {
                source,
                pattern,
                replacement,
                limit,
                index,
                output,
            } => match source.poll(*index, &mut control).map_err(|message| {
                AdapterError::UnsupportedValue(format!(
                    "invalid checked text replacement: {message}"
                ))
            })? {
                IterationPoll::Pending => Ok(NativeWorkPoll::Pending),
                IterationPoll::Ready(Some(InterpValue::String(segment))) => {
                    *output = Some(match output.take() {
                        None => segment,
                        Some(prefix) => {
                            let separator = if limit.is_none_or(|limit| *index <= limit) {
                                replacement
                            } else {
                                pattern
                            };
                            prefix
                                .concat_separated(segment, separator.as_str())
                                .map_err(|_| {
                                    AdapterError::Builtin(BuiltinError::Text(
                                        etas_builtin::TextFailure::LengthOverflow,
                                    ))
                                })?
                        }
                    });
                    *index += 1;
                    Ok(NativeWorkPoll::Pending)
                }
                IterationPoll::Ready(Some(_)) => Err(AdapterError::UnsupportedValue(
                    "text replacement produced a non-string segment".into(),
                )),
                IterationPoll::Ready(None) => Ok(NativeWorkPoll::Ready(InterpValue::String(
                    output.take().unwrap_or_default(),
                ))),
            },
            TextWorkState::ReplaceRange {
                source,
                source_cursor,
                range,
                replacement,
                replacement_cursor,
                phase,
                total_bytes,
                output,
            } => {
                output
                    .reserve_exact(*total_bytes, &mut control)
                    .map_err(AdapterError::Builtin)?;
                loop {
                    match phase {
                        ReplaceRangePhase::Prefix => {
                            if source_cursor.position() == range.start {
                                *phase = ReplaceRangePhase::Skip;
                                continue;
                            }
                            append_until(source_cursor, range.start, output, &mut control)?;
                            if source_cursor.position() != range.start {
                                return Ok(NativeWorkPoll::Pending);
                            }
                            *phase = ReplaceRangePhase::Skip;
                        }
                        ReplaceRangePhase::Skip => {
                            skip_until(source_cursor, range.end, &mut control)?;
                            if source_cursor.position() != range.end {
                                return Ok(NativeWorkPoll::Pending);
                            }
                            *phase = ReplaceRangePhase::Replacement;
                        }
                        ReplaceRangePhase::Replacement => {
                            append_until(
                                replacement_cursor,
                                replacement.len(),
                                output,
                                &mut control,
                            )?;
                            if !replacement_cursor.is_finished() {
                                return Ok(NativeWorkPoll::Pending);
                            }
                            *phase = ReplaceRangePhase::Suffix;
                        }
                        ReplaceRangePhase::Suffix => {
                            append_until(source_cursor, source.len(), output, &mut control)?;
                            if !source_cursor.is_finished() {
                                return Ok(NativeWorkPoll::Pending);
                            }
                            return Ok(NativeWorkPoll::Ready(InterpValue::String(
                                output.take().into(),
                            )));
                        }
                    }
                }
            }
            TextWorkState::Grapheme {
                source,
                scan,
                work,
                ordinal,
                requested,
                selected_start,
                selected_end,
                projection,
                output,
            } => loop {
                if let (Some(_), Some(start), Some(end)) =
                    (requested.as_ref(), *selected_start, *selected_end)
                {
                    output
                        .reserve_exact(end - start, &mut control)
                        .map_err(AdapterError::Builtin)?;
                    skip_until(projection, start, &mut control)?;
                    if projection.position() < start {
                        return Ok(NativeWorkPoll::Pending);
                    }
                    append_until(projection, end, output, &mut control)?;
                    if projection.position() != end {
                        return Ok(NativeWorkPoll::Pending);
                    }
                    return Ok(NativeWorkPoll::Ready(InterpValue::String(
                        output.take().into(),
                    )));
                }

                let (consumed, poll) = if let Some(piece) = scan.next_piece() {
                    let text = piece.as_str();
                    let end_of_input = scan.position() + text.len() == source.len();
                    work.step(text, end_of_input, &mut control)
                        .map_err(AdapterError::Builtin)?
                } else {
                    work.step("", true, &mut control)
                        .map_err(AdapterError::Builtin)?
                };
                scan.advance(consumed);
                match poll {
                    WorkPoll::Pending => {
                        if consumed == 0 || control.work_remaining() == 0 {
                            return Ok(NativeWorkPoll::Pending);
                        }
                    }
                    WorkPoll::Ready(etas_builtin::text::iteration::TextSequenceStep::Item(
                        etas_builtin::text::iteration::TextSequenceElement::Segment(range),
                    )) => {
                        *ordinal += 1;
                        if let Some(requested) = requested {
                            if requested.start == *ordinal {
                                *selected_start = Some(range.end);
                            }
                            if requested.end == *ordinal {
                                *selected_end = Some(range.end);
                            }
                        }
                    }
                    WorkPoll::Ready(etas_builtin::text::iteration::TextSequenceStep::End) => {
                        if requested.is_some() {
                            return Err(AdapterError::Builtin(BuiltinError::Text(
                                etas_builtin::TextFailure::InvalidRange,
                            )));
                        }
                        return from_builtin_for_type(
                            etas_builtin::BuiltinValue::Usize(*ordinal),
                            self.call.result_type,
                            projector,
                        )
                        .map(NativeWorkPoll::Ready);
                    }
                    WorkPoll::Ready(_) => {
                        return Err(AdapterError::UnsupportedValue(
                            "grapheme scanner produced an invalid element".into(),
                        ));
                    }
                }
            },
            TextWorkState::Scalar {
                source,
                scan,
                work,
                ordinal,
                operation,
                selected_start,
                selected_end,
                projection,
                output,
            } => loop {
                if let TextScalarOperation::Slice {
                    requested: None,
                    optional: true,
                } = operation
                {
                    return super::abi::text::from_optional_text_value(
                        None,
                        self.call.result_type,
                        projector,
                    )
                    .map(NativeWorkPoll::Ready);
                }
                if let TextScalarOperation::Slice {
                    requested: Some(_),
                    optional,
                } = operation
                    && let (Some(start), Some(end)) = (*selected_start, *selected_end)
                {
                    let value = if start == 0 && end == source.len() {
                        source.clone()
                    } else if start == end {
                        StringValue::default()
                    } else {
                        output
                            .reserve_exact(end - start, &mut control)
                            .map_err(AdapterError::Builtin)?;
                        skip_until(projection, start, &mut control)?;
                        if projection.position() < start {
                            return Ok(NativeWorkPoll::Pending);
                        }
                        append_until(projection, end, output, &mut control)?;
                        if projection.position() != end {
                            return Ok(NativeWorkPoll::Pending);
                        }
                        output.take().into()
                    };
                    return if *optional {
                        super::abi::text::from_optional_text_value(
                            Some(value),
                            self.call.result_type,
                            projector,
                        )
                    } else {
                        super::abi::text::from_text_value(value, self.call.result_type, projector)
                    }
                    .map(NativeWorkPoll::Ready);
                }

                let (consumed, poll) = if let Some(piece) = scan.next_piece() {
                    let text = piece.as_str();
                    let end_of_input = scan.position() + text.len() == source.len();
                    work.step(text, end_of_input, &mut control)
                        .map_err(AdapterError::Builtin)?
                } else {
                    work.step("", true, &mut control)
                        .map_err(AdapterError::Builtin)?
                };
                scan.advance(consumed);
                match poll {
                    WorkPoll::Pending => {
                        if consumed == 0 || control.work_remaining() == 0 {
                            return Ok(NativeWorkPoll::Pending);
                        }
                    }
                    WorkPoll::Ready(etas_builtin::text::iteration::TextSequenceStep::Item(
                        etas_builtin::text::iteration::TextSequenceElement::Scalar(scalar),
                    )) => {
                        if let TextScalarOperation::Get {
                            target: Some(target),
                        } = operation
                            && *target == *ordinal
                        {
                            return from_builtin_for_type(
                                etas_builtin::BuiltinValue::OptionSome(Box::new(
                                    etas_builtin::BuiltinValue::Char(scalar),
                                )),
                                self.call.result_type,
                                projector,
                            )
                            .map(NativeWorkPoll::Ready);
                        }
                        *ordinal += 1;
                        if let TextScalarOperation::Slice {
                            requested: Some(requested),
                            ..
                        } = operation
                        {
                            let boundary = work.cursor().byte_offset;
                            if requested.start == *ordinal {
                                *selected_start = Some(boundary);
                            }
                            if requested.end == *ordinal {
                                *selected_end = Some(boundary);
                            }
                        }
                    }
                    WorkPoll::Ready(etas_builtin::text::iteration::TextSequenceStep::End) => {
                        match operation {
                            TextScalarOperation::Get { .. } => {
                                return from_builtin_for_type(
                                    etas_builtin::BuiltinValue::OptionNone,
                                    self.call.result_type,
                                    projector,
                                )
                                .map(NativeWorkPoll::Ready);
                            }
                            TextScalarOperation::Slice { optional: true, .. } => {
                                return super::abi::text::from_optional_text_value(
                                    None,
                                    self.call.result_type,
                                    projector,
                                )
                                .map(NativeWorkPoll::Ready);
                            }
                            TextScalarOperation::Slice {
                                optional: false, ..
                            } => {
                                return Err(AdapterError::Builtin(BuiltinError::Text(
                                    etas_builtin::TextFailure::InvalidRange,
                                )));
                            }
                        }
                    }
                    WorkPoll::Ready(_) => {
                        return Err(AdapterError::UnsupportedValue(
                            "scalar scanner produced an invalid element".into(),
                        ));
                    }
                }
            },
            TextWorkState::ByteRange {
                source,
                scan,
                work,
                operation,
                projection,
                output,
            } => loop {
                match operation {
                    TextByteOperation::IsBoundary { target: None } => {
                        return from_builtin_for_type(
                            etas_builtin::BuiltinValue::Bool(false),
                            self.call.result_type,
                            projector,
                        )
                        .map(NativeWorkPoll::Ready);
                    }
                    TextByteOperation::IsBoundary {
                        target: Some(target),
                    } if *target == 0 || *target == source.len() => {
                        return from_builtin_for_type(
                            etas_builtin::BuiltinValue::Bool(true),
                            self.call.result_type,
                            projector,
                        )
                        .map(NativeWorkPoll::Ready);
                    }
                    TextByteOperation::Slice {
                        range: None,
                        optional: true,
                        ..
                    } => {
                        return super::abi::text::from_optional_text_value(
                            None,
                            self.call.result_type,
                            projector,
                        )
                        .map(NativeWorkPoll::Ready);
                    }
                    TextByteOperation::Slice {
                        range: Some(range),
                        optional,
                        start_valid: true,
                        end_valid: true,
                    } => {
                        let value = if range.start == 0 && range.end == source.len() {
                            source.clone()
                        } else if range.start == range.end {
                            StringValue::default()
                        } else {
                            output
                                .reserve_exact(range.len(), &mut control)
                                .map_err(AdapterError::Builtin)?;
                            skip_until(projection, range.start, &mut control)?;
                            if projection.position() < range.start {
                                return Ok(NativeWorkPoll::Pending);
                            }
                            append_until(projection, range.end, output, &mut control)?;
                            if projection.position() != range.end {
                                return Ok(NativeWorkPoll::Pending);
                            }
                            output.take().into()
                        };
                        return if *optional {
                            super::abi::text::from_optional_text_value(
                                Some(value),
                                self.call.result_type,
                                projector,
                            )
                        } else {
                            super::abi::text::from_text_value(
                                value,
                                self.call.result_type,
                                projector,
                            )
                        }
                        .map(NativeWorkPoll::Ready);
                    }
                    _ => {}
                }

                let (consumed, poll) = if let Some(piece) = scan.next_piece() {
                    let text = piece.as_str();
                    let end_of_input = scan.position() + text.len() == source.len();
                    work.step(text, end_of_input, &mut control)
                        .map_err(AdapterError::Builtin)?
                } else {
                    work.step("", true, &mut control)
                        .map_err(AdapterError::Builtin)?
                };
                scan.advance(consumed);
                match poll {
                    WorkPoll::Pending => {
                        if consumed == 0 || control.work_remaining() == 0 {
                            return Ok(NativeWorkPoll::Pending);
                        }
                    }
                    WorkPoll::Ready(etas_builtin::text::iteration::TextSequenceStep::Item(
                        etas_builtin::text::iteration::TextSequenceElement::Scalar(_),
                    )) => {
                        let boundary = work.cursor().byte_offset;
                        match operation {
                            TextByteOperation::IsBoundary {
                                target: Some(target),
                            } if boundary >= *target => {
                                return from_builtin_for_type(
                                    etas_builtin::BuiltinValue::Bool(boundary == *target),
                                    self.call.result_type,
                                    projector,
                                )
                                .map(NativeWorkPoll::Ready);
                            }
                            TextByteOperation::Slice {
                                range: Some(range),
                                optional,
                                start_valid,
                                end_valid,
                            } => {
                                if boundary == range.start {
                                    *start_valid = true;
                                } else if boundary > range.start && !*start_valid {
                                    return invalid_byte_range(
                                        *optional,
                                        self.call.result_type,
                                        projector,
                                    );
                                }
                                if boundary == range.end {
                                    *end_valid = true;
                                } else if boundary > range.end && !*end_valid {
                                    return invalid_byte_range(
                                        *optional,
                                        self.call.result_type,
                                        projector,
                                    );
                                }
                            }
                            _ => {}
                        }
                    }
                    WorkPoll::Ready(etas_builtin::text::iteration::TextSequenceStep::End) => {
                        return match operation {
                            TextByteOperation::IsBoundary { .. } => from_builtin_for_type(
                                etas_builtin::BuiltinValue::Bool(false),
                                self.call.result_type,
                                projector,
                            )
                            .map(NativeWorkPoll::Ready),
                            TextByteOperation::Slice { optional, .. } => {
                                invalid_byte_range(*optional, self.call.result_type, projector)
                            }
                        };
                    }
                    WorkPoll::Ready(_) => {
                        return Err(AdapterError::UnsupportedValue(
                            "byte-boundary scanner produced an invalid element".into(),
                        ));
                    }
                }
            },
            TextWorkState::AsciiEq {
                left,
                left_cursor,
                right,
                right_cursor,
            } => {
                if left.len() != right.len() {
                    return from_builtin_for_type(
                        etas_builtin::BuiltinValue::Bool(false),
                        self.call.result_type,
                        projector,
                    )
                    .map(NativeWorkPoll::Ready);
                }
                loop {
                    let (Some(left_piece), Some(right_piece)) =
                        (left_cursor.next_piece(), right_cursor.next_piece())
                    else {
                        return from_builtin_for_type(
                            etas_builtin::BuiltinValue::Bool(
                                left_cursor.is_finished() && right_cursor.is_finished(),
                            ),
                            self.call.result_type,
                            projector,
                        )
                        .map(NativeWorkPoll::Ready);
                    };
                    let bytes = left_piece
                        .as_str()
                        .len()
                        .min(right_piece.as_str().len())
                        .min(control.work_remaining());
                    if bytes == 0 {
                        return Ok(NativeWorkPoll::Pending);
                    }
                    control
                        .consume(bytes)
                        .expect("bounded ASCII comparison work");
                    if !left_piece.as_str().as_bytes()[..bytes]
                        .iter()
                        .zip(&right_piece.as_str().as_bytes()[..bytes])
                        .all(|(left, right)| left.eq_ignore_ascii_case(right))
                    {
                        return from_builtin_for_type(
                            etas_builtin::BuiltinValue::Bool(false),
                            self.call.result_type,
                            projector,
                        )
                        .map(NativeWorkPoll::Ready);
                    }
                    left_cursor.advance(bytes);
                    right_cursor.advance(bytes);
                    if control.work_remaining() == 0 {
                        return Ok(NativeWorkPoll::Pending);
                    }
                }
            }
            TextWorkState::Iteration { source, index } => {
                match source.poll(*index, &mut control).map_err(|message| {
                    AdapterError::UnsupportedValue(format!(
                        "invalid checked text iteration: {message}"
                    ))
                })? {
                    IterationPoll::Pending => Ok(NativeWorkPoll::Pending),
                    IterationPoll::Ready(value) => Ok(NativeWorkPoll::IterationReady(value)),
                }
            }
        }
    }

    fn output_reserved(&self) -> usize {
        match &self.state {
            TextWorkState::Search { output, .. }
            | TextWorkState::Repeat { output, .. }
            | TextWorkState::Join { output, .. }
            | TextWorkState::Pad { output, .. }
            | TextWorkState::ReplaceRange { output, .. }
            | TextWorkState::Grapheme { output, .. }
            | TextWorkState::Scalar { output, .. }
            | TextWorkState::ByteRange { output, .. } => output.reserved_bytes(),
            TextWorkState::Case { work, .. } => work.reserved_bytes(),
            TextWorkState::Casefold { work, .. } => work.reserved_bytes(),
            TextWorkState::Normalize { work, .. } => work.reserved_output_bytes(),
            TextWorkState::Reverse { work, .. } => work.reserved_output_bytes(),
            TextWorkState::Format { work, .. } => work.reserved_output_bytes(),
            TextWorkState::Query { .. }
            | TextWorkState::IntegerParse { .. }
            | TextWorkState::FloatParse { .. }
            | TextWorkState::AsciiEq { .. }
            | TextWorkState::Trim { .. } => 0,
            TextWorkState::Collect { source, .. } => source.native_output_reserved(),
            TextWorkState::Replace { source, .. } => source.native_output_reserved(),
            TextWorkState::Iteration { source, .. } => source.native_output_reserved(),
        }
    }

    fn scratch_reserved(&self) -> usize {
        match &self.state {
            TextWorkState::Search { search, .. } => search.reserved_scratch_bytes(),
            TextWorkState::Normalize { work, .. } => work.reserved_scratch_bytes(),
            TextWorkState::Query { .. }
            | TextWorkState::IntegerParse { .. }
            | TextWorkState::FloatParse { .. }
            | TextWorkState::AsciiEq { .. }
            | TextWorkState::Repeat { .. }
            | TextWorkState::Join { .. }
            | TextWorkState::Pad { .. }
            | TextWorkState::Format { .. }
            | TextWorkState::Case { .. }
            | TextWorkState::Trim { .. }
            | TextWorkState::Casefold { .. }
            | TextWorkState::ReplaceRange { .. } => 0,
            TextWorkState::Collect { source, .. } => source.native_scratch_reserved(),
            TextWorkState::Replace { source, .. } => source.native_scratch_reserved(),
            TextWorkState::Grapheme { work, .. } => work.reserved_scratch_bytes(),
            TextWorkState::Scalar { work, .. } => work.reserved_scratch_bytes(),
            TextWorkState::ByteRange { work, .. } => work.reserved_scratch_bytes(),
            TextWorkState::Reverse { work, .. } => work.reserved_scratch_bytes(),
            TextWorkState::Iteration { source, .. } => source.native_scratch_reserved(),
        }
    }

    pub(crate) fn call(&self) -> Option<&CheckedPureIntrinsicCall> {
        match &self.call {
            NativeWorkOwner::Intrinsic(call) => Some(call),
            NativeWorkOwner::ForLoop(_) => None,
        }
    }

    pub(crate) fn into_iteration(self) -> Option<(ForLoopResume, IterationSource, Frame)> {
        match (self.call, self.state) {
            (NativeWorkOwner::ForLoop(owner), TextWorkState::Iteration { source, .. }) => {
                Some((owner.resume, *source, owner.frame))
            }
            _ => None,
        }
    }
}

enum SearchCompletion {
    Query(etas_builtin::BuiltinValue),
    Missing,
    Range(std::ops::Range<usize>),
}

fn search_completion(
    finish: TextSearchFinish,
    value: etas_builtin::BuiltinValue,
    source_len: usize,
    pattern_len: usize,
) -> Result<SearchCompletion, AdapterError> {
    match finish {
        TextSearchFinish::Query => Ok(SearchCompletion::Query(value)),
        TextSearchFinish::StripPrefix | TextSearchFinish::StripSuffix => {
            let etas_builtin::BuiltinValue::Bool(found) = value else {
                return Err(AdapterError::UnsupportedValue(
                    "checked strip search did not produce a boolean".into(),
                ));
            };
            if !found {
                return Ok(SearchCompletion::Missing);
            }
            let range = match finish {
                TextSearchFinish::StripPrefix => pattern_len..source_len,
                TextSearchFinish::StripSuffix => 0..source_len - pattern_len,
                TextSearchFinish::Query => unreachable!(),
            };
            Ok(SearchCompletion::Range(range))
        }
    }
}

fn invalid_byte_range(
    optional: bool,
    result_type: TypeId,
    projector: &PureAbiProjector,
) -> Result<NativeWorkPoll, AdapterError> {
    if optional {
        super::abi::text::from_optional_text_value(None, result_type, projector)
            .map(NativeWorkPoll::Ready)
    } else {
        Err(AdapterError::Builtin(BuiltinError::Text(
            etas_builtin::TextFailure::NotCharBoundary,
        )))
    }
}

fn append_until(
    cursor: &mut OwnedTextCursor,
    end: usize,
    output: &mut TextOutputWork,
    control: &mut WorkControl,
) -> Result<(), AdapterError> {
    while cursor.position() < end {
        let piece = cursor.next_piece().ok_or_else(|| {
            AdapterError::UnsupportedValue("text cursor ended before its checked range".into())
        })?;
        let remaining = end - cursor.position();
        let input = &piece.as_str()[..remaining.min(piece.as_str().len())];
        let consumed = output
            .append_step(input, control)
            .map_err(AdapterError::Builtin)?;
        cursor.advance(consumed);
        if consumed != input.len() {
            break;
        }
    }
    Ok(())
}

fn skip_until(
    cursor: &mut OwnedTextCursor,
    end: usize,
    control: &mut WorkControl,
) -> Result<(), AdapterError> {
    while cursor.position() < end {
        let piece = cursor.next_piece().ok_or_else(|| {
            AdapterError::UnsupportedValue("text cursor ended before its checked range".into())
        })?;
        let remaining = end - cursor.position();
        let input = &piece.as_str()[..remaining.min(piece.as_str().len())];
        let mut consumed = 0usize;
        while let Some(scalar) = input[consumed..].chars().next() {
            let width = scalar.len_utf8();
            if control.consume(width).is_err() {
                cursor.advance(consumed);
                return Ok(());
            }
            consumed += width;
        }
        cursor.advance(consumed);
    }
    Ok(())
}

fn poll_text_collection(
    values: &mut Vec<InterpValue>,
    separator: &StringValue,
    kind: TextCollectKind,
    reduction: &mut Option<TextCollectionReduction>,
    control: &mut WorkControl,
) -> Result<WorkPoll<InterpValue>, AdapterError> {
    if reduction.is_none() {
        match kind {
            TextCollectKind::Split => {
                return Ok(WorkPoll::Ready(InterpValue::Array(ArrayValue::new(
                    std::mem::take(values),
                ))));
            }
            TextCollectKind::SplitN(count) => {
                if count == 0 {
                    return Ok(WorkPoll::Ready(InterpValue::Array(ArrayValue::new(
                        Vec::new(),
                    ))));
                }
                if values.len() > count {
                    let remainder = values.split_off(count - 1);
                    *reduction = Some(TextCollectionReduction {
                        values: remainder,
                        index: 0,
                        output: None,
                        finish: TextCollectionFinish::SplitN(std::mem::take(values)),
                    });
                } else {
                    return Ok(WorkPoll::Ready(InterpValue::Array(ArrayValue::new(
                        std::mem::take(values),
                    ))));
                }
            }
            TextCollectKind::SplitOnce => {
                if values.len() < 2 {
                    return Ok(WorkPoll::Ready(InterpValue::OptionNone));
                }
                let right = values.split_off(1);
                let left = values.pop().expect("split collection has a first segment");
                *reduction = Some(TextCollectionReduction {
                    values: right,
                    index: 0,
                    output: None,
                    finish: TextCollectionFinish::SplitOnce(left),
                });
            }
            TextCollectKind::RsplitOnce => {
                if values.len() < 2 {
                    return Ok(WorkPoll::Ready(InterpValue::OptionNone));
                }
                let right = values.pop().expect("split collection has a final segment");
                *reduction = Some(TextCollectionReduction {
                    values: std::mem::take(values),
                    index: 0,
                    output: None,
                    finish: TextCollectionFinish::RsplitOnce(right),
                });
            }
        }
    }

    let reduction = reduction
        .as_mut()
        .expect("text collection reduction was initialized above");
    while reduction.index < reduction.values.len() {
        if control.consume(1).is_err() {
            return Ok(WorkPoll::Pending);
        }
        let InterpValue::String(value) = reduction.values[reduction.index].clone() else {
            return Err(AdapterError::UnsupportedValue(
                "text collection produced a non-string segment".into(),
            ));
        };
        reduction.output = Some(match reduction.output.take() {
            None => value,
            Some(output) => output
                .concat_separated(value, separator.as_str())
                .map_err(|_| AdapterError::Builtin(BuiltinError::ResourceExhausted))?,
        });
        reduction.index += 1;
    }
    let output = reduction.output.take().unwrap_or_default();
    let finish = reduction.finish.clone();
    *reduction = TextCollectionReduction {
        values: Vec::new(),
        index: 0,
        output: None,
        finish: finish.clone(),
    };
    Ok(WorkPoll::Ready(match finish {
        TextCollectionFinish::SplitN(mut prefix) => {
            prefix.push(InterpValue::String(output));
            InterpValue::Array(ArrayValue::new(prefix))
        }
        TextCollectionFinish::SplitOnce(left) => InterpValue::OptionSome(
            InterpValue::Tuple(vec![left, InterpValue::String(output)].into()).into(),
        ),
        TextCollectionFinish::RsplitOnce(right) => InterpValue::OptionSome(
            InterpValue::Tuple(vec![InterpValue::String(output), right].into()).into(),
        ),
    }))
}
