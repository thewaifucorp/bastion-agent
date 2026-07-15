import argparse
import asyncio
import json
from datetime import UTC, datetime

from .factory import Settings, create_adapter


async def async_main():
    parser = argparse.ArgumentParser(description="Life Log CLI")
    subparsers = parser.add_subparsers(dest="command", required=True)

    # Command: log
    log_parser = subparsers.add_parser("log")
    log_parser.add_argument("--persona", required=True)
    log_parser.add_argument("--intent", required=True)
    log_parser.add_argument("--tools", default="[]")
    log_parser.add_argument("--embedding", default="[]")

    # Command: search
    search_parser = subparsers.add_parser("search")
    search_parser.add_argument("--query_embedding", default="[]")
    search_parser.add_argument("--persona", default=None)
    search_parser.add_argument("--limit", type=int, default=5)
    search_parser.add_argument("--threshold", type=float, default=0.65)

    # Command: summary
    summary_parser = subparsers.add_parser("summary")
    summary_parser.add_argument("--persona", required=True)
    summary_parser.add_argument("--days", type=int, default=7)

    args = parser.parse_args()

    settings = Settings.from_env()
    adapter = create_adapter(settings)

    if args.command == "log":
        tools_list = json.loads(args.tools)
        # Fake embedding if empty
        embedding_list = json.loads(args.embedding)
        if not embedding_list:
            embedding_list = [0.0] * 1536

        record_id = await adapter.log_interaction(
            persona=args.persona,
            intent=args.intent,
            tools=tools_list,
            embedding=embedding_list,
            timestamp=datetime.now(tz=UTC)
        )
        print(f"Logged interaction: {record_id}")

    elif args.command == "search":
        query_emb = json.loads(args.query_embedding)
        if not query_emb:
            query_emb = [0.0] * 1536
        results = await adapter.search_similar(
            query_embedding=query_emb,
            persona=args.persona,
            limit=args.limit,
            threshold=args.threshold
        )
        for r in results:
            print(f"- {r.timestamp}: [{r.intent}]")

    elif args.command == "summary":
        results = await adapter.get_persona_summary(
            persona=args.persona,
            days=args.days
        )
        print(f"Summary for {args.persona} (last {args.days} days):")
        for r in results:
            print(f"- {r.timestamp}: [{r.intent}]")

if __name__ == "__main__":
    asyncio.run(async_main())
