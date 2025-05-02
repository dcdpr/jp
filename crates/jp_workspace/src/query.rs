use jp_conversation::{Conversation, ConversationId};

pub struct ConversationQuery<'a, 'b> {
    pub active_conversation_id: ConversationId,
    pub conversations: &'a mut dyn Iterator<Item = (&'b ConversationId, &'b Conversation)>,
}

impl<'a, 'b> ConversationQuery<'a, 'b> {
    pub fn new(
        active_conversation_id: ConversationId,
        conversations: &'a mut (dyn Iterator<Item = (&'b ConversationId, &'b Conversation)> + 'b),
    ) -> Self {
        Self {
            active_conversation_id,
            conversations,
        }
    }

    pub fn all(&'b self) -> &'b dyn Iterator<Item = (&'a ConversationId, &'a Conversation)> {
        self.conversations
    }

    pub fn last_active_conversation(&mut self) -> Option<&'a Conversation> {
        self.last_active_conversation_with_id()
            .map(|(_, conversation)| conversation)
    }

    pub fn last_active_conversation_id(&mut self) -> Option<&'a ConversationId> {
        self.last_active_conversation_with_id().map(|(id, _)| id)
    }

    fn last_active_conversation_with_id(
        &mut self,
    ) -> Option<(&'a ConversationId, &'a Conversation)> {
        self.conversations
            .filter(|(id, _)| **id != self.active_conversation_id)
            .max_by_key(|(_, conversation)| conversation.last_activated_at)
    }
}
