use super::memory::dispatch_memory_request;
use crate::{eval::EvalContext, host::HostServices};
use etas_host::{
    HostError, HostErrorCode, MemoryOperation, MemoryRequest, MemoryResponse, MemoryResult,
    StorageLimits,
};
use std::collections::BTreeSet;

// Legacy list operations require a complete, bounded scan, not a single page.
pub(super) async fn dispatch_read(
    eval: &mut EvalContext<'_>,
    host: &dyn HostServices,
    mut request: MemoryRequest,
) -> Result<MemoryResponse, HostError> {
    if let MemoryOperation::Scan {
        ref cursor,
        limit: Some(limit),
    } = request.operation
    {
        let previous = cursor.as_ref().map(|cursor| cursor.opaque.clone());
        let response = dispatch_memory_request(eval, host, request).await?;
        if let Ok(result) = &response.result {
            validate_page(result, previous.as_deref(), limit, &eval.storage_limits)?;
        }
        return Ok(response);
    }
    if !matches!(request.operation, MemoryOperation::Scan { limit: None, .. }) {
        return dispatch_memory_request(eval, host, request).await;
    }
    let id = request.id;
    let limits = eval.storage_limits.clone();
    let mut collected = Vec::new();
    let mut bytes = 0usize;
    let mut cursors = BTreeSet::new();
    if let MemoryOperation::Scan {
        cursor: Some(cursor),
        ..
    } = &request.operation
    {
        cursors.insert(cursor.opaque.clone());
    }
    loop {
        request.budget.check_time()?;
        let response = dispatch_memory_request(eval, host, request.clone()).await?;
        let MemoryResult::Entries { entries, cursor } = response.result? else {
            return Err(HostError::new(
                HostErrorCode::InvalidResponse,
                "memory scan did not return entries",
            ));
        };
        if collected.len().saturating_add(entries.len()) > limits.max_scan_rows {
            return Err(exceeded());
        }
        for entry in &entries {
            let size = limits
                .value_size(&entry.key)?
                .checked_add(limits.value_size(&entry.value)?)
                .and_then(|n| n.checked_add(entry.version.as_token().len()))
                .ok_or_else(exceeded)?;
            bytes = bytes.checked_add(size).ok_or_else(exceeded)?;
            if bytes > limits.max_result_bytes {
                return Err(exceeded());
            }
        }
        if let Some(cursor) = &cursor {
            if entries.is_empty()
                || cursor.opaque.len() > limits.max_value_bytes
                || !cursors.insert(cursor.opaque.clone())
            {
                return Err(HostError::new(
                    HostErrorCode::InvalidResponse,
                    "memory scan cursor made no progress",
                ));
            }
        }
        collected.extend(entries);
        let Some(cursor) = cursor else {
            return Ok(MemoryResponse {
                id,
                result: Ok(MemoryResult::Entries {
                    entries: collected,
                    cursor: None,
                }),
            });
        };
        request.id = eval.next_host_request_id();
        request.operation = MemoryOperation::Scan {
            cursor: Some(cursor),
            limit: None,
        };
    }
}

fn validate_page(
    result: &MemoryResult,
    previous: Option<&str>,
    limit: u32,
    limits: &StorageLimits,
) -> Result<(), HostError> {
    let MemoryResult::Entries { entries, cursor } = result else {
        return Err(HostError::new(
            HostErrorCode::InvalidResponse,
            "memory page did not return entries",
        ));
    };
    if limit == 0 || entries.len() > limit as usize || entries.len() > limits.max_scan_rows {
        return Err(HostError::new(
            HostErrorCode::InvalidResponse,
            "memory page exceeds the requested row limit",
        ));
    }
    let mut bytes = 0usize;
    for entry in entries {
        bytes = bytes
            .checked_add(limits.value_size(&entry.key)?)
            .and_then(|n| n.checked_add(entry.version.as_token().len()))
            .ok_or_else(exceeded)?;
        bytes = bytes
            .checked_add(limits.value_size(&entry.value)?)
            .ok_or_else(exceeded)?;
        if bytes > limits.max_result_bytes {
            return Err(exceeded());
        }
    }
    if let Some(cursor) = cursor {
        if entries.is_empty()
            || cursor.opaque.is_empty()
            || Some(cursor.opaque.as_str()) == previous
        {
            return Err(HostError::new(
                HostErrorCode::InvalidResponse,
                "memory page cursor made no progress",
            ));
        }
        if cursor.opaque.len() > limits.max_value_bytes
            || bytes.saturating_add(cursor.opaque.len()) > limits.max_result_bytes
        {
            return Err(exceeded());
        }
    }
    Ok(())
}
fn exceeded() -> HostError {
    HostError::new(
        HostErrorCode::BudgetExceeded,
        "complete memory scan exceeds interpreter collection limits",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_response_rejects_excess_rows_and_nonadvancing_cursor() {
        let entry = etas_host::MemoryEntry {
            key: etas_host::HostValue::String("key".into()),
            value: etas_host::HostValue::String("value".into()),
            version: etas_host::MemoryVersion::parse(&format!(
                "mv1:{}:{}:0000000000000001",
                "0".repeat(64),
                "0".repeat(32)
            ))
            .unwrap(),
        };
        let page = |entries, token: Option<&str>| MemoryResult::Entries {
            entries,
            cursor: token.map(|opaque| etas_host::MemoryCursor {
                opaque: opaque.into(),
            }),
        };
        assert!(
            validate_page(
                &page(vec![entry.clone(), entry.clone()], None),
                None,
                1,
                &StorageLimits::default()
            )
            .is_err()
        );
        assert!(
            validate_page(
                &page(vec![], Some("next")),
                None,
                1,
                &StorageLimits::default()
            )
            .is_err()
        );
        assert!(
            validate_page(
                &page(vec![entry.clone()], Some("same")),
                Some("same"),
                1,
                &StorageLimits::default()
            )
            .is_err()
        );
        assert!(
            validate_page(
                &page(vec![entry], Some("next")),
                Some("prior"),
                1,
                &StorageLimits::default()
            )
            .is_ok()
        );
    }
}
