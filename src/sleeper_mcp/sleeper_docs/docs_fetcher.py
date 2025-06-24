"""Sleeper API documentation fetcher and cache manager."""

import aiohttp
import os
from datetime import datetime
from bs4 import BeautifulSoup
from pathlib import Path
from typing import Optional

class SleeperDocsFetcher:
    """Fetches and caches Sleeper API documentation."""
    
    BASE_URL = "https://docs.sleeper.app"
    CACHE_DIR = Path("cached_docs")
    
    def __init__(self):
        """Initialize the fetcher and ensure cache directory exists."""
        self.CACHE_DIR.mkdir(exist_ok=True)
    
    async def fetch_and_cache_docs(self) -> str:
        """Fetches Sleeper API docs and caches them locally.
        
        Returns:
            str: The fetched HTML content
        """
        async with aiohttp.ClientSession() as session:
            async with session.get(self.BASE_URL) as response:
                response.raise_for_status()
                content = await response.text()
                
                # Cache raw HTML with timestamp
                timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
                cache_path = self.CACHE_DIR / f"sleeper_docs_{timestamp}.html"
                
                cache_path.write_text(content, encoding="utf-8")
                
                # Update latest symlink
                latest_link = self.CACHE_DIR / "latest.html"
                if latest_link.exists():
                    latest_link.unlink()
                latest_link.symlink_to(cache_path.name)
                
                return content
    
    def get_cached_docs(self, version: Optional[str] = None) -> Optional[str]:
        """Retrieves cached documentation.
        
        Args:
            version: Optional timestamp version (YYYYMMDD_HHMMSS format).
                    If None, returns latest version.
        
        Returns:
            Optional[str]: The cached HTML content or None if not found
        """
        if version:
            cache_path = self.CACHE_DIR / f"sleeper_docs_{version}.html"
        else:
            cache_path = self.CACHE_DIR / "latest.html"
        
        try:
            return cache_path.read_text(encoding="utf-8")
        except (FileNotFoundError, OSError):
            return None
    
    def list_cached_versions(self) -> list[str]:
        """Lists all cached documentation versions.
        
        Returns:
            list[str]: List of version timestamps (YYYYMMDD_HHMMSS format)
        """
        return sorted([
            f.stem.replace("sleeper_docs_", "")
            for f in self.CACHE_DIR.glob("sleeper_docs_*.html")
        ])
