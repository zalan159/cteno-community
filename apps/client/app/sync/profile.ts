import * as z from 'zod';

//
// Schema
//

export const GitHubProfileSchema = z.object({
    id: z.number(),
    login: z.string(),
    name: z.string(),
    avatar_url: z.string(),
    email: z.string().optional(),
    bio: z.string().nullable()
});

export const WechatProfileSchema = z.object({
    openid: z.string(),
    unionid: z.string().optional(),
    nickname: z.string(),
    headimgurl: z.string(),
    sex: z.number().optional(),
    language: z.string().optional(),
    city: z.string().optional(),
    province: z.string().optional(),
    country: z.string().optional(),
});

export const ImageRefSchema = z.object({
    width: z.number(),
    height: z.number(),
    thumbhash: z.string(),
    path: z.string(),
    url: z.string()
});

export const ProfileSchema = z.object({
    id: z.string(),
    timestamp: z.number(),
    firstName: z.string().nullable(),
    lastName: z.string().nullable(),
    email: z.string().nullable().default(null),
    username: z.string().nullable().default(null),
    avatar: ImageRefSchema.nullable(),
    github: GitHubProfileSchema.nullable(),
    wechat: WechatProfileSchema.nullable().default(null),
    connectedServices: z.array(z.string()).default([])
});

export type GitHubProfile = z.infer<typeof GitHubProfileSchema>;
export type WechatProfile = z.infer<typeof WechatProfileSchema>;
export type ImageRef = z.infer<typeof ImageRefSchema>;
export type Profile = z.infer<typeof ProfileSchema>;

//
// Defaults
//

export const profileDefaults: Profile = {
    id: '',
    timestamp: 0,
    firstName: null,
    lastName: null,
    email: null,
    username: null,
    avatar: null,
    github: null,
    wechat: null,
    connectedServices: []
};
Object.freeze(profileDefaults);

//
// Parsing
//

export function profileParse(profile: unknown): Profile {
    const parsed = ProfileSchema.safeParse(profile);
    if (!parsed.success) {
        console.error('Failed to parse profile:', parsed.error);
        return { ...profileDefaults };
    }
    return parsed.data;
}

//
// Utility functions
//

export function getDisplayName(profile: Profile): string | null {
    if (profile.firstName || profile.lastName) {
        return [profile.firstName, profile.lastName].filter(Boolean).join(' ');
    }
    if (profile.username) {
        return profile.username;
    }
    if (profile.wechat?.nickname) {
        return profile.wechat.nickname;
    }
    if (profile.github?.name) {
        return profile.github.name;
    }
    if (profile.github?.login) {
        return profile.github.login;
    }
    if (profile.email) {
        return profile.email;
    }
    return null;
}

export function getAvatarUrl(profile: Profile): string | null {
    if (profile.avatar?.url) {
        return profile.avatar.url;
    }
    if (profile.github?.avatar_url) {
        return profile.github.avatar_url;
    }
    return null;
}

export function getBio(profile: Profile): string | null {
    return profile.github?.bio || null;
}