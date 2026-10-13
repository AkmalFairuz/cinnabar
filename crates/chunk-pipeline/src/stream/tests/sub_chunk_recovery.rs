use super::*;

/// Times out every attempt at `request`'s sub-chunks from `sent`, through the last retry, and
/// returns when the stream abandons them.
fn abandon(
    stream: &mut WorldStream,
    mut request: super::super::PendingSubChunkRequest,
    mut sent: Instant,
) -> Instant {
    for _ in 0..=super::super::MAX_SUB_CHUNK_RETRIES {
        acknowledge_request_sent(stream, &request, sent);
        sent += super::super::SUB_CHUNK_RESPONSE_TIMEOUT;
        stream.expire_sub_chunk_deadlines(sent);
        match stream.pop_next_request() {
            Some(retry) => request = retry,
            None => break,
        }
    }
    sent
}

/// A server too busy to answer in time once left a permanent hole: the abandoned sub-chunk was
/// never asked for again and its column never had collision, which froze a player beside it.
#[test]
fn an_abandoned_sub_chunk_is_requested_again_until_it_arrives() {
    let started = Instant::now();
    let (mut stream, keys, request) = stream_with_unsent_sub_chunks(1);
    let chunk = keys[0].chunk();
    let abandoned = abandon(&mut stream, request, started);
    assert_eq!(stream.stats().sub_chunk_retry_exhaustions, 1);
    assert!(!stream.loaded_columns.contains(&chunk));

    stream.request_due_sub_chunk_recoveries(abandoned);
    assert!(
        stream.pop_next_request().is_none(),
        "asked again only after a delay"
    );
    let due = abandoned + super::super::SUB_CHUNK_RECOVERY_DELAY;
    stream.request_due_sub_chunk_recoveries(due);
    let again = stream
        .pop_next_request()
        .expect("the abandoned sub-chunk is requested again");
    assert_eq!(
        (again.chunk, again.base_sub_chunk_y, again.count),
        (chunk, keys[0].y, 1)
    );

    acknowledge_request_sent(&mut stream, &again, due);
    apply_sub_chunk_result(
        &mut stream,
        keys[0],
        super::super::PreparedSubChunkResult::AllAir,
    );
    assert!(
        stream.loaded_columns.contains(&chunk),
        "the column now has collision"
    );
    assert!(stream.requests.recoveries.is_empty());
}

/// Each abandonment waits twice as long as the last before the next request.
#[test]
fn repeated_abandonment_backs_off() {
    let started = Instant::now();
    let (mut stream, keys, request) = stream_with_unsent_sub_chunks(1);
    let abandoned = abandon(&mut stream, request, started);
    stream.request_due_sub_chunk_recoveries(abandoned);
    let first = abandoned + super::super::SUB_CHUNK_RECOVERY_DELAY;
    stream.request_due_sub_chunk_recoveries(first);
    let again = stream.pop_next_request().expect("first recovery");
    let abandoned_again = abandon(&mut stream, again, first);
    assert_eq!(stream.requests.recoveries[&keys[0]].rounds, 2);

    stream.request_due_sub_chunk_recoveries(abandoned_again);
    stream
        .request_due_sub_chunk_recoveries(abandoned_again + super::super::SUB_CHUNK_RECOVERY_DELAY);
    assert!(
        stream.pop_next_request().is_none(),
        "the second wait is longer"
    );
    stream.request_due_sub_chunk_recoveries(
        abandoned_again + super::super::SUB_CHUNK_RECOVERY_DELAY * 2,
    );
    assert!(stream.pop_next_request().is_some());
}
