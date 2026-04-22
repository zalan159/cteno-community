import { z } from 'zod';
import { GitHubProfileSchema, ImageRefSchema, WechatProfileSchema } from './profile';
import { RelationshipStatusSchema, UserProfileSchema } from './friendTypes';
import { FeedBodySchema } from './feedTypes';

//
// Encrypted message
//

export const ApiMessageSchema = z.object({
    id: z.string(),
    seq: z.number(),
    localId: z.string().nullish(),
    content: z.discriminatedUnion('t', [
        z.object({
            t: z.literal('encrypted'),
            c: z.string(), // Base64 encoded encrypted content
        }),
        z.object({
            t: z.literal('plaintext'),
            c: z.string(), // Plain JSON string
        }),
    ]),
    createdAt: z.number(),
});

export type ApiMessage = z.infer<typeof ApiMessageSchema>;

//
// Updates
//

export const ApiUpdateNewMessageSchema = z.object({
    t: z.literal('new-message'),
    sid: z.string(), // Session ID
    message: ApiMessageSchema
});

export const ApiUpdateNewSessionSchema = z.object({
    t: z.literal('new-session'),
    id: z.string(), // Session ID
    createdAt: z.number(),
    updatedAt: z.number(),
});

export const ApiDeleteSessionSchema = z.object({
    t: z.literal('delete-session'),
    sid: z.string(), // Session ID
});

export const ApiUpdateSessionStateSchema = z.object({
    t: z.literal('update-session'),
    id: z.string(),
    agentState: z.object({
        version: z.number(),
        value: z.string()
    }).nullish(),
    metadata: z.object({
        version: z.number(),
        value: z.string()
    }).nullish(),
});

export const ApiUpdateAccountSchema = z.object({
    t: z.literal('update-account'),
    id: z.string(),
    settings: z.object({
        value: z.string().nullish(),
        version: z.number()
    }).nullish(),
    firstName: z.string().nullish(),
    lastName: z.string().nullish(),
    avatar: ImageRefSchema.nullish(),
    github: GitHubProfileSchema.nullish(),
    wechat: WechatProfileSchema.nullish(),
});

export const ApiUpdateMachineStateSchema = z.object({
    t: z.literal('update-machine'),
    machineId: z.string(),  // Changed from 'id' to 'machineId' for clarity
    metadata: z.object({
        version: z.number(),
        value: z.string() // Encrypted, client decrypts
    }).nullish(),
    daemonState: z.object({
        version: z.number(),
        value: z.string() // Encrypted, client decrypts
    }).nullish(),
    active: z.boolean().optional(),
    activeAt: z.number().optional()
});

// Relationship update schema
export const ApiRelationshipUpdatedSchema = z.object({
    t: z.literal('relationship-updated'),
    fromUserId: z.string(),
    toUserId: z.string(),
    status: RelationshipStatusSchema,
    action: z.enum(['created', 'updated', 'deleted']),
    fromUser: UserProfileSchema.optional(),
    toUser: UserProfileSchema.optional(),
    timestamp: z.number()
});

// Feed update schema
export const ApiNewFeedPostSchema = z.object({
    t: z.literal('new-feed-post'),
    id: z.string(),
    body: FeedBodySchema,
    cursor: z.string(),
    createdAt: z.number(),
    repeatKey: z.string().nullable()
});

// KV batch update schema for real-time KV updates
export const ApiKvBatchUpdateSchema = z.object({
    t: z.literal('kv-batch-update'),
    changes: z.array(z.object({
        key: z.string(),
        value: z.string().nullable(),
        version: z.number()
    }))
});

export const ApiUpdateSchema = z.discriminatedUnion('t', [
    ApiUpdateNewMessageSchema,
    ApiUpdateNewSessionSchema,
    ApiDeleteSessionSchema,
    ApiUpdateSessionStateSchema,
    ApiUpdateAccountSchema,
    ApiUpdateMachineStateSchema,
    ApiRelationshipUpdatedSchema,
    ApiNewFeedPostSchema,
    ApiKvBatchUpdateSchema
]);

export type ApiUpdateNewMessage = z.infer<typeof ApiUpdateNewMessageSchema>;
export type ApiRelationshipUpdated = z.infer<typeof ApiRelationshipUpdatedSchema>;
export type ApiKvBatchUpdate = z.infer<typeof ApiKvBatchUpdateSchema>;
export type ApiUpdate = z.infer<typeof ApiUpdateSchema>;

//
// API update container
//

export const ApiUpdateContainerSchema = z.object({
    id: z.string(),
    seq: z.number(),
    body: ApiUpdateSchema,
    createdAt: z.number(),
});

export type ApiUpdateContainer = z.infer<typeof ApiUpdateContainerSchema>;

//
// Ephemeral update
//

export const ApiEphemeralActivityUpdateSchema = z.object({
    type: z.literal('activity'),
    id: z.string(),
    active: z.boolean(),
    activeAt: z.number(),
    thinking: z.boolean(),
    thinkingStatus: z.string().optional(),
});

export const ApiEphemeralUsageUpdateSchema = z.object({
    type: z.literal('usage'),
    id: z.string(),
    key: z.string(),
    timestamp: z.number(),
    tokens: z.object({
        total: z.number(),
        input: z.number(),
        output: z.number(),
        cache_creation: z.number(),
        cache_read: z.number(),
    }),
    cost: z.object({
        total: z.number(),
        input: z.number(),
        output: z.number(),
    }),
});

export const ApiEphemeralMachineActivityUpdateSchema = z.object({
    type: z.literal('machine-activity'),
    id: z.string(), // machine id
    active: z.boolean(),
    activeAt: z.number(),
});

export const ApiEphemeralAgentPushSchema = z.object({
    type: z.literal('hypothesis-push'), // kept as 'hypothesis-push' for wire-compat
    agentId: z.string(),
    event: z.string(), // 'a2ui_updated' | 'persona_session_ready' etc.
}).passthrough(); // allow extra fields

export const ApiEphemeralUpdateSchema = z.union([
    ApiEphemeralActivityUpdateSchema,
    ApiEphemeralUsageUpdateSchema,
    ApiEphemeralMachineActivityUpdateSchema,
    ApiEphemeralAgentPushSchema,
]);

export type ApiEphemeralActivityUpdate = z.infer<typeof ApiEphemeralActivityUpdateSchema>;
export type ApiEphemeralUpdate = z.infer<typeof ApiEphemeralUpdateSchema>;

// Machine metadata updates use Partial<MachineMetadata> from storageTypes
// This matches how session metadata updates work
