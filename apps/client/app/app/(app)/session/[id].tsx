import * as React from 'react';
import { useRoute } from "@react-navigation/native";
import { SessionPersonaPage } from '@/app/(app)/persona/[id]';


export default React.memo(() => {
    const route = useRoute();
    const sessionId = (route.params! as any).id as string;
    return (<SessionPersonaPage id={sessionId} />);
});
