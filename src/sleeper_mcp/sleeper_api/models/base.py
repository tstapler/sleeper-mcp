"""Pydantic models for Sleeper API data."""

from pydantic import BaseModel, Field
from typing import Optional, List, Dict, Any

class User(BaseModel):
    """Sleeper user model."""
    
    user_id: str
    username: str
    display_name: Optional[str] = None
    avatar: Optional[str] = None
    is_bot: bool = False
    metadata: Optional[Dict[str, Any]] = None

class RosterSettings(BaseModel):
    """League roster settings model."""
    
    wins: int = 0
    waiver_position: int = 0
    waiver_budget_used: int = 0
    total_moves: int = 0
    ties: int = 0
    losses: int = 0
    fpts: float = 0.0
    fpts_decimal: int = 0
    fpts_against: float = 0.0

class Roster(BaseModel):
    """League roster model."""
    
    roster_id: int
    owner_id: Optional[str] = None
    league_id: str
    settings: RosterSettings
    players: Optional[List[str]] = None
    starters: Optional[List[str]] = None
    reserve: Optional[List[str]] = None
    metadata: Optional[Dict[str, Any]] = None

class LeagueSettings(BaseModel):
    """League settings model."""
    
    playoff_teams: int = Field(default=6, ge=0)
    name: Optional[str] = None
    max_keepers: Optional[int] = None
    draft_rounds: int = Field(default=15, ge=0)
    daily_waivers: Optional[bool] = None
    waiver_type: Optional[int] = None
    waiver_day_of_week: Optional[int] = None
    start_week: Optional[int] = None
    playoff_week_start: Optional[int] = None
    daily_waivers_hour: Optional[int] = None
    waiver_clear_days: Optional[int] = None
    waiver_budget: Optional[int] = None
    reserve_slots: Optional[int] = None
    trade_deadline: Optional[int] = None
    trade_review_days: Optional[int] = None

class League(BaseModel):
    """Sleeper league model."""
    
    league_id: str
    name: str
    season: str
    status: str
    sport: str
    total_rosters: int
    settings: LeagueSettings
    scoring_settings: Dict[str, Any]
    roster_positions: List[str]
    previous_league_id: Optional[str] = None
    draft_id: Optional[str] = None
    metadata: Optional[Dict[str, Any]] = None
