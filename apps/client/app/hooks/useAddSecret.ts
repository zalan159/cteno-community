import * as React from 'react';

export function useAddSecret() {
    const [isAdding, setIsAdding] = React.useState(false);

    const addImportedSecret = React.useCallback(async (_secretString: string, _label?: string) => {
        setIsAdding(true);
        try {
            throw new Error('Device secret import is no longer supported. Use browser login instead.');
        } catch (error) {
            console.error('Failed to add imported secret:', error);
            throw error;
        } finally {
            setIsAdding(false);
        }
    }, []);

    return { addImportedSecret, isAdding };
}
